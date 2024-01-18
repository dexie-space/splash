use clap::Parser;
use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use libp2p::{identify, identity, Multiaddr, PeerId, StreamProtocol};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{io, select, time};
use tracing_subscriber::EnvFilter;
use warp::Filter;

#[derive(NetworkBehaviour)]
struct SplashBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
}

const BOOTNODES: [&str; 3] = [
    "12D3KooWM1So76jzugAettgrfA1jfcaKA66EAE6k1zwAT3oVzcnK",
    "12D3KooWCLvBXPohyMUKhbRrkcfRRkMLDfnCqyCjNSk6qyfjLMJ8",
    "12D3KooWP6QDYTCccwfUQVAc6jQDvzVY1FtU3WVsAxmVratbbC5V",
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();
    let (offer_tx, mut offer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    let id_keys = match opt.identity_file {
        Some(ref file_path) if fs::metadata(file_path).is_ok() => {
            load_keypair_from_file(file_path)?
        }
        _ => {
            let keypair = identity::Keypair::generate_ed25519();
            if let Some(ref file_path) = opt.identity_file {
                save_keypair_to_file(&keypair, file_path)?;
            }
            keypair
        }
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(id_keys)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_dns()?
        .with_behaviour(|key| {
            println!("Our Peer ID: {}", key.public().to_peer_id().to_string());

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
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?; // Temporary hack because `build` does not return a proper `std::error::Error`.

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

            // Create a Kademlia behaviour.
            let mut cfg = kad::Config::default();

            cfg.set_protocol_names(vec![StreamProtocol::try_from_owned(format!(
                "/splash/kad/1"
            ))?]);

            cfg.set_query_timeout(Duration::from_secs(60));
            let store = kad::store::MemoryStore::new(key.public().to_peer_id());

            let mut kademlia = kad::Behaviour::with_config(key.public().to_peer_id(), store, cfg);

            // In case the user provided an known peer, use it to enter the network
            if let Some(addr) = opt.known_peer {
                let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                    return Err("Expect peer multiaddr to contain peer ID.".into());
                };

                kademlia.add_address(&peer_id, addr);
            } else {
                println!("No known peers, bootstrapping from dexies dns introducer");
                for peer in &BOOTNODES {
                    kademlia.add_address(&peer.parse()?, "/dnsaddr/splash.dexie.space".parse()?);
                }
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

    if !opt.listen_address.is_empty() {
        for addr in opt.listen_address.iter() {
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
            if let Some(offer_str) = offer.get("offer").and_then(|v| v.as_str()) {
                // Convert the offer string to bytes and send
                let offer_bytes = offer_str.as_bytes().to_vec();
                let tx = offer_tx_clone.clone();
                tokio::spawn(async move {
                    if tx.send(offer_bytes).await.is_err() {
                        eprintln!("Failed to send offer through the channel");
                    }
                });
                warp::reply::with_status("Offer received", warp::http::StatusCode::OK)
            } else {
                warp::reply::with_status(
                    "Invalid offer format",
                    warp::http::StatusCode::BAD_REQUEST,
                )
            }
        });

    // Start the warp server using the address provided in the `listen_offer_submission` option.
    if let Some(submission_addr_str) = opt.listen_offer_submission {
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
                    println!("Connected to peer: {peer_id}");
                },
                SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    println!("Disconnected from peer: {peer_id}");
                },
                SwarmEvent::Behaviour(SplashBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: _,
                    message_id: _,
                    message,
                })) => {
                    let data_clone = message.data.clone();
                    let msg_str = String::from_utf8_lossy(&data_clone).into_owned();

                    if msg_str.starts_with("offer1") {
                        println!(
                            "Received Offer: {}",
                            msg_str,
                        );

                        if let Some(ref endpoint_url) = opt.offer_hook {
                            let endpoint_url_clone = endpoint_url.clone();
                            tokio::spawn(async move {
                                if let Err(e) = offer_post_hook(&endpoint_url_clone, &msg_str).await {
                                    eprintln!("Error posting to offer hook: {}", e);
                                }
                            });
                        }
                    }
                },
                SwarmEvent::Behaviour(SplashBehaviourEvent::Identify(identify::Event::Received { info: identify::Info { observed_addr, listen_addrs, .. }, peer_id })) => {
                    for addr in listen_addrs {
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                    }
                    // Mark the address observed for us by the external peer as confirmed.
                    // TODO: We shouldn't trust this, instead we should confirm our own address manually or using
                    // `libp2p-autonat`.
                    swarm.add_external_address(observed_addr);
                },
                SwarmEvent::NewListenAddr { address, .. } => {
                    println!("Listening on: {address}");
                },
                _ => {}
            }
        }
    }
}

async fn offer_post_hook(endpoint: &str, offer: &str) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new();

    let offer_json = json!({ "offer": offer });
    client.post(endpoint).json(&offer_json).send().await?;

    Ok(())
}

fn save_keypair_to_file(keypair: &identity::Keypair, file_path: &str) -> io::Result<()> {
    let encoded = keypair.to_protobuf_encoding().unwrap();
    let keypair_json: IdentityJson = IdentityJson { identity: encoded };
    let json = serde_json::to_string(&keypair_json)?;
    let mut file = File::create(file_path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn load_keypair_from_file(file_path: &str) -> io::Result<identity::Keypair> {
    let contents = fs::read_to_string(file_path)?;
    let keypair_json: IdentityJson = serde_json::from_str(&contents)?;
    identity::Keypair::from_protobuf_encoding(&keypair_json.identity)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid keypair data"))
}

#[derive(Serialize, Deserialize)]
struct IdentityJson {
    identity: Vec<u8>,
}

#[derive(Parser, Debug)]
#[clap(name = "Splash!")]
struct Opt {
    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set initial peer, if missing use dexies DNS introducer"
    )]
    known_peer: Option<Multiaddr>,

    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set listen address, defaults to all interfaces, use multiple times for multiple addresses"
    )]
    listen_address: Vec<Multiaddr>,

    #[clap(
        long,
        short,
        help = "Store and reuse peer identity (only useful for known peers)"
    )]
    identity_file: Option<String>,

    #[clap(
        long,
        help = "HTTP endpoint where incoming offers are posted to, sends JSON body {\"offer\":\"offer1...\"} (defaults to STDOUT)"
    )]
    offer_hook: Option<String>,

    #[clap(
        long,
        help = "Start a HTTP API for offer submission, expects JSON body {\"offer\":\"offer1...\"}",
        value_name = "HOST:PORT"
    )]
    listen_offer_submission: Option<String>,
}
