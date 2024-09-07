use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use libp2p::{identify, identity, Multiaddr, PeerId, StreamProtocol};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::{io, select, time};
mod dns;

const MAX_OFFER_SIZE: usize = 300 * 1024;

#[derive(Error, Debug)]
pub enum SplashError {
    #[error("Offer exceeds maximum size of {0} bytes")]
    OfferTooLarge(usize),
    #[error("Invalid offer format: not a valid bech32 string")]
    InvalidOfferFormat,
    #[error("Failed to send offer to network")]
    SendError,
}

pub enum SplashEvent {
    Initialized(PeerId),
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    OfferReceived(String),
    NewListenAddress(Multiaddr),
    OfferBroadcasted(String),
    OfferBroadcastFailed(gossipsub::PublishError),
}

pub struct Splash {
    pub listen_addresses: Vec<Multiaddr>,
    pub known_peers: Vec<Multiaddr>,
    pub keys: identity::Keypair,
    submission: Sender<Vec<u8>>,
    submission_receiver: Option<Receiver<Vec<u8>>>,
}

pub struct SplashContext {
    pub node: Splash,
    pub events: mpsc::Receiver<SplashEvent>,
}

impl Clone for Splash {
    fn clone(&self) -> Self {
        Splash {
            listen_addresses: self.listen_addresses.clone(),
            known_peers: self.known_peers.clone(),
            keys: self.keys.clone(),
            submission: self.submission.clone(),
            submission_receiver: None,
        }
    }
}

#[derive(NetworkBehaviour)]
struct SplashBehaviour {
    gossipsub: gossipsub::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
}

impl Splash {
    pub fn new() -> Splash {
        let (submission_sender, submission_receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

        Splash {
            known_peers: Vec::new(),
            listen_addresses: Vec::new(),
            keys: identity::Keypair::generate_ed25519(),
            submission: submission_sender,
            submission_receiver: Some(submission_receiver),
        }
    }

    pub async fn submit_offer(&self, offer: &str) -> Result<(), SplashError> {
        if offer.len() > MAX_OFFER_SIZE {
            return Err(SplashError::OfferTooLarge(MAX_OFFER_SIZE));
        }

        if bech32::decode(&offer).is_err() {
            return Err(SplashError::InvalidOfferFormat);
        }

        self.submission
            .send(offer.as_bytes().to_vec())
            .await
            .map_err(|_| SplashError::SendError)?;

        Ok(())
    }

    pub fn with_listen_addresses(mut self, listen_addresses: Vec<Multiaddr>) -> Self {
        self.listen_addresses = listen_addresses;
        self
    }

    pub fn with_known_peers(mut self, known_peers: Vec<Multiaddr>) -> Self {
        self.known_peers = known_peers;
        self
    }

    pub fn with_keys(mut self, keys: identity::Keypair) -> Self {
        self.keys = keys;
        self
    }

    pub async fn build(mut self) -> Result<SplashContext, Box<dyn std::error::Error>> {
        let (event_tx, event_rx) = mpsc::channel(100);

        // Check if known_peers is empty and resolve from DNS if necessary
        if self.known_peers.is_empty() {
            self.known_peers = dns::resolve_peers_from_dns()
                .await
                .map_err(|e| format!("Failed to resolve peers from DNS: {}", e))?;
        }

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(self.keys.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
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

                let mut kademlia =
                    kad::Behaviour::with_config(key.public().to_peer_id(), store, cfg);

                for addr in self.known_peers.iter() {
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

        if !self.listen_addresses.is_empty() {
            for addr in self.listen_addresses.iter() {
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

        let mut peer_discovery_interval = time::interval(time::Duration::from_secs(10));

        // Take submission_receiver early to avoid partial move error
        let mut submission_receiver = self
            .submission_receiver
            .take()
            .ok_or("Submission receiver already consumed")?;

        event_tx
            .send(SplashEvent::Initialized(self.keys.public().to_peer_id()))
            .await
            .ok();

        // Main event loop
        tokio::spawn(async move {
            loop {
                select! {
                    Some(offer) = submission_receiver.recv() => {

                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), offer.clone()) {
                            event_tx.send(SplashEvent::OfferBroadcastFailed(e)).await.ok();
                        }

                        event_tx.send(SplashEvent::OfferBroadcasted(String::from_utf8_lossy(&offer).to_string())).await.ok();
                    },
                    _ = peer_discovery_interval.tick() => {
                        swarm.behaviour_mut().kademlia.get_closest_peers(PeerId::random());
                    },
                    event = swarm.select_next_some() => match event {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                            event_tx.send(SplashEvent::PeerConnected(peer_id)).await.ok();
                        },
                        SwarmEvent::ConnectionClosed { peer_id, .. } => {
                            event_tx.send(SplashEvent::PeerDisconnected(peer_id)).await.ok();
                        },
                        SwarmEvent::Behaviour(SplashBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                            propagation_source: _,
                            message_id: _,
                            message,
                        })) => {
                            let msg_str = String::from_utf8_lossy(&message.data).into_owned();

                            // TODO: are we really keeping the sanity check this simple?
                            if msg_str.starts_with("offer1") {
                                event_tx.send(SplashEvent::OfferReceived(msg_str)).await.ok();
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
                            event_tx.send(SplashEvent::NewListenAddress(address)).await.ok();
                        },
                        _ => {}
                    }
                }
            }
        });

        Ok(SplashContext {
            node: self,
            events: event_rx,
        })
    }
}
