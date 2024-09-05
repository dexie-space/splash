use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use libp2p::{identify, identity, Multiaddr, PeerId, StreamProtocol};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::{io, select, time};
use tracing_subscriber::EnvFilter;
use warp::http::header;
use warp::http::StatusCode;
use warp::Filter;

mod dns;

pub enum SplashEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    OfferReceived(String),
    NewListenAddress(Multiaddr),
}

#[derive(NetworkBehaviour)]
struct SplashBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
}

const MAX_OFFER_SIZE: usize = 300 * 1024;

pub async fn run_splash(
    known_peers: Vec<Multiaddr>,
    listen_addresses: Vec<Multiaddr>,
    listen_offer_submission: Option<String>,
    keys: Option<identity::Keypair>,
    event_tx: mpsc::Sender<SplashEvent>,
) -> Result<(), Box<dyn Error>> {
    let keys = keys.unwrap_or_else(identity::Keypair::generate_ed25519);

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let (offer_tx, mut offer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    let known_peers = if known_peers.is_empty() {
        println!("No known peers, bootstrapping from dexies dns introducer");
        dns::resolve_peers_from_dns()
            .await
            .map_err(|e| format!("Failed to resolve peers from dns: {}", e))?
    } else {
        known_peers.clone()
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keys)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            println!("Our Peer ID: {}", key.public().to_peer_id());

            // We can take the hash of message and use it as an ID.
            let unique_offer_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(5)) // This is set to aid debugging by not cluttering the log space
                .message_id_fn(unique_offer_fn) // No duplicate offers will be propagated.
                .max_transmit_size(MAX_OFFER_SIZE)
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?; // Temporary hack because `build` does not return a proper `std::error::Error`.

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

            // Create a Kademlia behaviour.
            let mut cfg = kad::Config::new(
                StreamProtocol::try_from_owned("/splash/kad/1".to_string())
                    .expect("protocol name is valid"),
            );

            cfg.set_query_timeout(Duration::from_secs(60));
            let store = kad::store::MemoryStore::new(key.public().to_peer_id());

            let mut kademlia = kad::Behaviour::with_config(key.public().to_peer_id(), store, cfg);

            for addr in known_peers.iter() {
                let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                    return Err("Expect peer multiaddr to contain peer ID.".into());
                };
                kademlia.add_address(&peer_id, addr.clone());
            }

            kademlia.bootstrap().unwrap();

            let identify = identify::Behaviour::new(identify::Config::new(
                "/splash/id/1".into(),
                key.public().clone(),
            ));

            Ok(SplashBehaviour {
                gossipsub,
                kademlia,
                identify,
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    if !listen_addresses.is_empty() {
        for addr in listen_addresses.iter() {
            swarm.listen_on(addr.clone())?;
        }
    } else {
        // Fallback to default addresses if no listen addresses are provided
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        swarm.listen_on("/ip6/::/tcp/0".parse()?)?;
    }

    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new("/splash/offers/1");

    // subscribes to our topic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    let offer_tx_clone = offer_tx.clone();

    let offer_route = warp::post()
        .and(warp::body::json())
        .map(move |offer: serde_json::Value| {
            let response = match offer.get("offer").and_then(|v| v.as_str()) {
                Some(offer_str) if offer_str.as_bytes().len() > MAX_OFFER_SIZE => {
                    warp::reply::with_status(
                        warp::reply::json(&json!({
                            "success": false,
                            "error": "Offer too large"
                        })),
                        StatusCode::BAD_REQUEST,
                    )
                }
                Some(offer_str) if bech32::decode(offer_str).is_ok() => {
                    let offer_bytes = offer_str.as_bytes().to_vec();
                    let tx = offer_tx_clone.clone();
                    tokio::spawn(async move {
                        if tx.send(offer_bytes).await.is_err() {
                            eprintln!("Failed to send offer through the channel");
                        }
                    });
                    warp::reply::with_status(
                        warp::reply::json(&json!({
                            "success": true,
                        })),
                        StatusCode::OK,
                    )
                }
                _ => warp::reply::with_status(
                    warp::reply::json(&json!({
                        "success": false,
                        "error": "Invalid offer format"
                    })),
                    StatusCode::BAD_REQUEST,
                ),
            };

            warp::reply::with_header(response, header::CONTENT_TYPE, "application/json")
        });

    // Start the warp server using the address provided in the `listen_offer_submission` option.
    if let Some(submission_addr_str) = listen_offer_submission {
        let submission_addr: SocketAddr =
            submission_addr_str.parse().expect("Invalid socket address");
        tokio::spawn(async move {
            warp::serve(offer_route).run(submission_addr).await;
        });
    }

    let mut peer_discovery_interval = time::interval(time::Duration::from_secs(10));

    loop {
        select! {
            Some(offer) = offer_rx.recv() => {
                println!("Broadcasting Offer: {}", String::from_utf8_lossy(&offer));

                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), offer) {
                    eprintln!("Broadcasting offer failed: {}", e);
                }
            },
            _ = peer_discovery_interval.tick() => {
                swarm.behaviour_mut().kademlia.get_closest_peers(PeerId::random());
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    let _ = event_tx.send(SplashEvent::PeerConnected(peer_id)).await;
                },
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    let _ = event_tx.send(SplashEvent::PeerDisconnected(peer_id)).await;
                },
                SwarmEvent::Behaviour(SplashBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: _,
                    message_id: _,
                    message,
                })) => {
                    let data_clone = message.data.clone();
                    let msg_str = String::from_utf8_lossy(&data_clone).into_owned();

                    // TODO: are we really keeping the sanity check this simple?
                    if msg_str.starts_with("offer1") {
                        let _ = event_tx.send(SplashEvent::OfferReceived(msg_str)).await;
                    }
                },
                SwarmEvent::Behaviour(SplashBehaviourEvent::Identify(identify::Event::Received { info: identify::Info { observed_addr, listen_addrs, .. }, peer_id, connection_id: _ })) => {
                    for addr in listen_addrs {
                        // If the node is advertising a non-global address, ignore it
                        // TODO: also filter out ipv6 private addresses when rust API is finalized
                        let is_non_global = addr.iter().any(|p| match p {
                            Protocol::Ip4(addr) => addr.is_loopback() || addr.is_private(),
                            Protocol::Ip6(addr) => addr.is_loopback(),
                            _ => false,
                        });

                        if is_non_global {
                            continue;
                        }

                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                    }
                    // Mark the address observed for us by the external peer as confirmed.
                    // TODO: We shouldn't trust this, instead we should confirm our own address manually or using
                    // `libp2p-autonat`.
                    swarm.add_external_address(observed_addr);
                },
                SwarmEvent::NewListenAddr { address, .. } => {
                    let _ = event_tx.send(SplashEvent::NewListenAddress(address)).await;
                },
                _ => {}
            }
        }
    }
}
