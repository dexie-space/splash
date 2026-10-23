use bech32::{primitives::decode::UncheckedHrpstring, Bech32, Bech32m, Checksum};
use futures::stream::StreamExt;
use libp2p::gossipsub::MessageAcceptance;
use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, kad, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use libp2p::{identify, identity, Multiaddr, PeerId, StreamProtocol};
use log::warn;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::{select, time};
mod dns;

const MAX_OFFER_SIZE: usize = 100 * 1024;

// Chia offers can exceed Bech32's standard 1023-character code length. Preserve
// the old decoder's acceptance of long strings, bounded by MAX_OFFER_SIZE, while
// keeping the original Bech32/Bech32m checksum algorithms unchanged.
struct OfferChecksum<Ck>(PhantomData<Ck>);

impl<Ck: Checksum> Checksum for OfferChecksum<Ck> {
    type MidstateRepr = Ck::MidstateRepr;
    type CorrectionField = Ck::CorrectionField;

    const ROOT_GENERATOR: Self::CorrectionField = Ck::ROOT_GENERATOR;
    const ROOT_EXPONENTS: std::ops::RangeInclusive<usize> = Ck::ROOT_EXPONENTS;
    const CODE_LENGTH: usize = MAX_OFFER_SIZE;
    const CHECKSUM_LENGTH: usize = Ck::CHECKSUM_LENGTH;
    const GENERATOR_SH: [Self::MidstateRepr; 5] = Ck::GENERATOR_SH;
    const TARGET_RESIDUE: Self::MidstateRepr = Ck::TARGET_RESIDUE;
}

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
    network_name: String,
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
            network_name: self.network_name.clone(),
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

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl Splash {
    pub fn new() -> Splash {
        let (submission_sender, submission_receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

        Splash {
            known_peers: Vec::new(),
            listen_addresses: Vec::new(),
            keys: identity::Keypair::generate_ed25519(),
            network_name: "splash".to_string(),
            submission: submission_sender,
            submission_receiver: Some(submission_receiver),
        }
    }

    pub fn validate_offer(offer: &str) -> Result<(), SplashError> {
        if offer.len() > MAX_OFFER_SIZE {
            return Err(SplashError::OfferTooLarge(MAX_OFFER_SIZE));
        }

        let decoded =
            UncheckedHrpstring::new(offer).map_err(|_| SplashError::InvalidOfferFormat)?;
        if !decoded.has_valid_checksum::<OfferChecksum<Bech32>>()
            && !decoded.has_valid_checksum::<OfferChecksum<Bech32m>>()
        {
            return Err(SplashError::InvalidOfferFormat);
        }

        // TODO: more validations?

        Ok(())
    }

    pub async fn broadcast_offer(&self, offer: &str) -> Result<(), SplashError> {
        Splash::validate_offer(offer)?;

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

    pub fn with_testnet(mut self) -> Self {
        self.network_name = "splash-testnet".to_string();
        self
    }

    pub async fn build(mut self) -> Result<SplashContext, Box<dyn std::error::Error>> {
        let (event_tx, event_rx) = mpsc::channel(100);

        // Check if known_peers is empty and resolve from DNS if necessary
        if self.known_peers.is_empty() {
            self.known_peers = dns::resolve_peers_from_dns(self.network_name.clone())
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
                    .validate_messages()
                    .validation_mode(gossipsub::ValidationMode::Permissive)
                    .build()?;

                // Generate a dummy keypair for signing gossipsub messages
                // TODO: use gossipsub::MessageAuthenticity::RandomAuthor and disable signing for even more privacy (once enough nodes are updated)
                let dummy_key = identity::Keypair::generate_ed25519();

                // build a gossipsub network behaviour
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(dummy_key),
                    gossipsub_config,
                )?;

                // Create a Kademlia behaviour.
                let mut cfg = kad::Config::new(
                    StreamProtocol::try_from_owned(format!("/{}/kad/1", self.network_name))
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

                let identify = identify::Behaviour::new(
                    identify::Config::new(
                        format!("/{}/id/1", self.network_name),
                        key.public().clone(),
                    )
                    .with_agent_version(format!("splash/{}", env!("CARGO_PKG_VERSION"))),
                );

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
        let topic = gossipsub::IdentTopic::new(format!("/{}/offers/1", self.network_name));

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
                            propagation_source,
                            message_id,
                            message,
                        })) => {
                            let msg_str = String::from_utf8_lossy(&message.data).into_owned();

                            match Splash::validate_offer(&msg_str) {
                                Ok(_) => {
                                    event_tx.send(SplashEvent::OfferReceived(msg_str)).await.ok();
                                    swarm.behaviour_mut().gossipsub.report_message_validation_result(&message_id, &propagation_source, MessageAcceptance::Accept);
                                }
                                Err(e) => {
                                    warn!("Received invalid offer: {}", e);
                                    swarm.behaviour_mut().gossipsub.report_message_validation_result(&message_id, &propagation_source, MessageAcceptance::Reject);
                                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bech32::{Fe32, Fe32IterExt, Hrp};

    fn offer_with_length<Ck: Checksum>(length: usize) -> String {
        let hrp = Hrp::parse("offer").unwrap();
        let data_length = length - hrp.as_bytes().len() - 1 - Ck::CHECKSUM_LENGTH;
        std::iter::repeat_n(Fe32::Q, data_length)
            .with_checksum::<Ck>(&hrp)
            .chars()
            .collect()
    }

    #[test]
    fn accepts_both_checksum_variants_and_alternative_prefixes() {
        for prefix in ["offer", "offer_v2", "other"] {
            let hrp = Hrp::parse(prefix).unwrap();
            let original = bech32::encode::<Bech32>(hrp, &[1, 2, 3]).unwrap();
            let modified = bech32::encode::<Bech32m>(hrp, &[1, 2, 3]).unwrap();
            assert!(Splash::validate_offer(&original).is_ok());
            assert!(Splash::validate_offer(&modified).is_ok());
            assert!(Splash::validate_offer(&original.to_uppercase()).is_ok());
        }
    }

    #[test]
    fn accepts_legacy_checksum_vectors() {
        for offer in ["A12UEL5L", "a12uel5l", "A1LQFN3A", "a1lqfn3a"] {
            assert!(Splash::validate_offer(offer).is_ok(), "{offer}");
        }
    }

    #[test]
    fn accepts_long_offers_for_both_checksum_variants() {
        for offer in [
            offer_with_length::<OfferChecksum<Bech32>>(4096),
            offer_with_length::<OfferChecksum<Bech32m>>(4096),
        ] {
            assert!(bech32::decode(&offer).is_err());
            assert!(Splash::validate_offer(&offer).is_ok());
        }
    }

    #[test]
    fn accepts_offers_at_the_size_limit() {
        let offer = offer_with_length::<OfferChecksum<Bech32>>(MAX_OFFER_SIZE);
        assert_eq!(offer.len(), MAX_OFFER_SIZE);
        assert!(Splash::validate_offer(&offer).is_ok());
    }

    #[test]
    fn rejects_offers_one_byte_over_the_size_limit() {
        let offer = offer_with_length::<OfferChecksum<Bech32>>(MAX_OFFER_SIZE + 1);
        assert!(matches!(
            Splash::validate_offer(&offer),
            Err(SplashError::OfferTooLarge(MAX_OFFER_SIZE))
        ));
    }

    #[test]
    fn rejects_oversized_offers_before_decoding() {
        assert!(matches!(
            Splash::validate_offer(&"q".repeat(MAX_OFFER_SIZE + 1)),
            Err(SplashError::OfferTooLarge(MAX_OFFER_SIZE))
        ));
    }

    #[test]
    fn rejects_malformed_offers() {
        for offer in [
            "",
            "offer1",
            "offer1invalid",
            "A12uEL5L",
            "a12uel5q",
            "🦀1qqqqqq",
        ] {
            assert!(
                matches!(
                    Splash::validate_offer(offer),
                    Err(SplashError::InvalidOfferFormat)
                ),
                "{offer}"
            );
        }
    }

    #[test]
    fn offer_checksums_retain_the_standard_parameters() {
        OfferChecksum::<Bech32>::sanity_check();
        OfferChecksum::<Bech32m>::sanity_check();
    }
}
