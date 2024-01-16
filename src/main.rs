use clap::Parser;
use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use libp2p::{identify, identity, Multiaddr, StreamProtocol};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::time::Duration;
use tokio::{io, io::AsyncBufReadExt, select};
use tracing_subscriber::EnvFilter;

#[derive(NetworkBehaviour)]
struct SplashBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

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

            // In case the user provided an address of a peer on the CLI, dial it.
            if let Some(addr) = opt.introducer {
                let Some(Protocol::P2p(peer_id)) = addr.iter().last() else {
                    return Err("Expect peer multiaddr to contain peer ID.".into());
                };

                kademlia.add_address(&peer_id, addr);
                kademlia.bootstrap().unwrap();
            }

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

    if let Some(addr) = opt.listen_address {
        swarm.listen_on(addr.clone())?;
    } else {
        swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        swarm.listen_on("/ip6/::/tcp/0".parse()?)?;
    }

    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new("/splash/offers/1");

    // subscribes to our topic
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    loop {
        select! {
            Ok(Some(line)) = stdin.next_line() => {
                if let Err(e) = swarm
                    .behaviour_mut().gossipsub
                    .publish(topic.clone(), line.as_bytes()) {
                    println!("Publish error: {e:?}");
                }
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("Connected to peer: {peer_id}");
                }
                SwarmEvent::Behaviour(SplashBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: _,
                    message_id: _,
                    message,
                })) => {
                    let msg_str = String::from_utf8_lossy(&message.data);
                    if msg_str.starts_with("offer1") {
                        println!(
                            "Received Offer: {}",
                            msg_str,
                        );
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
    #[clap(long)]
    introducer: Option<Multiaddr>,

    #[clap(long)]
    listen_address: Option<Multiaddr>,

    #[clap(long)]
    identity_file: Option<String>,
}
