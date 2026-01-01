//! libp2p NetworkBehaviour implementation
//!
//! Combines multiple protocols into a single behaviour:
//! - Kademlia for peer discovery
//! - Identify for peer info exchange
//! - Ping for liveness checking

use libp2p::{
    identify, kad, kad::store::MemoryStore, ping, swarm::NetworkBehaviour, Multiaddr, PeerId,
};
use std::time::Duration;
use tracing::{debug, info};

/// Protocol name for CyxCloud Kademlia DHT
pub const KAD_PROTOCOL: &str = "/cyxcloud/kad/1.0.0";

/// Protocol name for CyxCloud Identify
pub const IDENTIFY_PROTOCOL: &str = "/cyxcloud/id/1.0.0";

/// Agent version string
pub const AGENT_VERSION: &str = concat!("cyxcloud/", env!("CARGO_PKG_VERSION"));

/// Combined network behaviour for CyxCloud nodes
#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "CyxCloudEvent")]
pub struct CyxCloudBehaviour {
    /// Kademlia DHT for peer discovery
    pub kademlia: kad::Behaviour<MemoryStore>,
    /// Identify protocol for exchanging peer info
    pub identify: identify::Behaviour,
    /// Ping protocol for liveness checking
    pub ping: ping::Behaviour,
}

/// Events emitted by the CyxCloud behaviour
#[derive(Debug)]
pub enum CyxCloudEvent {
    /// A new peer was discovered
    PeerDiscovered {
        peer_id: PeerId,
        addresses: Vec<Multiaddr>,
    },
    /// A peer has expired (not seen for a while)
    PeerExpired { peer_id: PeerId },
    /// Ping result received
    PingResult { peer_id: PeerId, rtt: Duration },
    /// Ping failed
    PingFailed { peer_id: PeerId },
    /// Identify info received
    IdentifyReceived {
        peer_id: PeerId,
        info: identify::Info,
    },
    /// Kademlia event
    Kademlia(kad::Event),
}

impl From<kad::Event> for CyxCloudEvent {
    fn from(event: kad::Event) -> Self {
        match &event {
            kad::Event::RoutingUpdated { peer, .. } => {
                debug!(peer = %peer, "Kademlia routing updated");
            }
            kad::Event::OutboundQueryProgressed { result, .. } => {
                debug!(?result, "Kademlia query progressed");
            }
            _ => {}
        }
        CyxCloudEvent::Kademlia(event)
    }
}

impl From<identify::Event> for CyxCloudEvent {
    fn from(event: identify::Event) -> Self {
        match event {
            identify::Event::Received { peer_id, info } => {
                info!(
                    peer = %peer_id,
                    agent = %info.agent_version,
                    protocols = ?info.protocols,
                    "Identify info received"
                );
                CyxCloudEvent::IdentifyReceived { peer_id, info }
            }
            identify::Event::Sent { peer_id } => {
                debug!(peer = %peer_id, "Identify info sent");
                // No event for sent, just return a placeholder
                CyxCloudEvent::PeerDiscovered {
                    peer_id,
                    addresses: Vec::new(),
                }
            }
            identify::Event::Pushed { peer_id, .. } => {
                debug!(peer = %peer_id, "Identify info pushed");
                CyxCloudEvent::PeerDiscovered {
                    peer_id,
                    addresses: Vec::new(),
                }
            }
            identify::Event::Error { peer_id, error } => {
                debug!(peer = %peer_id, error = %error, "Identify error");
                CyxCloudEvent::PeerExpired { peer_id }
            }
        }
    }
}

impl From<ping::Event> for CyxCloudEvent {
    fn from(event: ping::Event) -> Self {
        match event.result {
            Ok(rtt) => {
                debug!(peer = %event.peer, rtt_ms = rtt.as_millis(), "Ping success");
                CyxCloudEvent::PingResult {
                    peer_id: event.peer,
                    rtt,
                }
            }
            Err(e) => {
                debug!(peer = %event.peer, error = %e, "Ping failed");
                CyxCloudEvent::PingFailed {
                    peer_id: event.peer,
                }
            }
        }
    }
}

/// Configuration for the CyxCloud behaviour
#[derive(Debug, Clone)]
pub struct BehaviourConfig {
    /// Local peer ID
    pub local_peer_id: PeerId,
    /// Local public key (for identify protocol)
    pub local_public_key: libp2p::identity::PublicKey,
    /// Ping interval
    pub ping_interval: Duration,
    /// Ping timeout
    pub ping_timeout: Duration,
    /// Kademlia query timeout
    pub kademlia_query_timeout: Duration,
    /// Kademlia record replication interval
    pub kademlia_replication_interval: Duration,
}

impl BehaviourConfig {
    /// Create a new config from a keypair
    pub fn from_keypair(keypair: &libp2p::identity::Keypair) -> Self {
        Self {
            local_peer_id: keypair.public().to_peer_id(),
            local_public_key: keypair.public(),
            ping_interval: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(10),
            kademlia_query_timeout: Duration::from_secs(60),
            kademlia_replication_interval: Duration::from_secs(3600), // 1 hour
        }
    }

    /// Set ping interval
    pub fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Set ping timeout
    pub fn with_ping_timeout(mut self, timeout: Duration) -> Self {
        self.ping_timeout = timeout;
        self
    }
}

impl CyxCloudBehaviour {
    /// Create a new CyxCloud behaviour
    pub fn new(config: BehaviourConfig) -> Self {
        // Create Kademlia behaviour
        let store = MemoryStore::new(config.local_peer_id);
        let mut kad_config = kad::Config::default();
        kad_config.set_query_timeout(config.kademlia_query_timeout);
        kad_config.set_replication_interval(Some(config.kademlia_replication_interval));
        kad_config.set_protocol_names(vec![libp2p::StreamProtocol::try_from_owned(
            KAD_PROTOCOL.to_string(),
        )
        .unwrap()]);

        let kademlia = kad::Behaviour::with_config(config.local_peer_id, store, kad_config);

        // Create Identify behaviour
        let identify_config = identify::Config::new(
            IDENTIFY_PROTOCOL.to_string(),
            config.local_public_key.clone(),
        )
        .with_agent_version(AGENT_VERSION.to_string());
        let identify = identify::Behaviour::new(identify_config);

        // Create Ping behaviour
        let ping_config = ping::Config::new()
            .with_interval(config.ping_interval)
            .with_timeout(config.ping_timeout);
        let ping = ping::Behaviour::new(ping_config);

        info!(
            peer_id = %config.local_peer_id,
            ping_interval = ?config.ping_interval,
            "CyxCloud behaviour initialized"
        );

        Self {
            kademlia,
            identify,
            ping,
        }
    }

    /// Add a bootstrap peer to Kademlia
    pub fn add_address(&mut self, peer_id: &PeerId, addr: Multiaddr) {
        self.kademlia.add_address(peer_id, addr);
        debug!(peer = %peer_id, "Added peer address to Kademlia");
    }

    /// Start a Kademlia bootstrap
    pub fn bootstrap(&mut self) -> Result<kad::QueryId, kad::NoKnownPeers> {
        info!("Starting Kademlia bootstrap");
        self.kademlia.bootstrap()
    }

    /// Put a record into the DHT
    pub fn put_record(
        &mut self,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<kad::QueryId, kad::store::Error> {
        let record = kad::Record {
            key: kad::RecordKey::new(&key),
            value,
            publisher: None,
            expires: None,
        };
        self.kademlia.put_record(record, kad::Quorum::One)
    }

    /// Get a record from the DHT
    pub fn get_record(&mut self, key: Vec<u8>) -> kad::QueryId {
        let key = kad::RecordKey::new(&key);
        self.kademlia.get_record(key)
    }

    /// Get the closest peers to a key
    pub fn get_closest_peers(&mut self, key: Vec<u8>) -> kad::QueryId {
        let key = kad::RecordKey::new(&key);
        self.kademlia.get_closest_peers(key.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    #[test]
    fn test_behaviour_creation() {
        let keypair = Keypair::generate_ed25519();
        let config = BehaviourConfig::from_keypair(&keypair);
        let behaviour = CyxCloudBehaviour::new(config);

        // Verify the behaviour was created
        assert!(behaviour.kademlia.iter_queries().next().is_none());
    }

    #[test]
    fn test_config_builder() {
        let keypair = Keypair::generate_ed25519();

        let config = BehaviourConfig::from_keypair(&keypair)
            .with_ping_interval(Duration::from_secs(60))
            .with_ping_timeout(Duration::from_secs(20));

        assert_eq!(config.ping_interval, Duration::from_secs(60));
        assert_eq!(config.ping_timeout, Duration::from_secs(20));
    }

    #[test]
    fn test_add_address() {
        let keypair = Keypair::generate_ed25519();
        let config = BehaviourConfig::from_keypair(&keypair);
        let mut behaviour = CyxCloudBehaviour::new(config);

        let other_keypair = Keypair::generate_ed25519();
        let other_peer_id = other_keypair.public().to_peer_id();
        let addr: Multiaddr = "/ip4/127.0.0.1/udp/4001/quic-v1".parse().unwrap();

        behaviour.add_address(&other_peer_id, addr);
        // Address should be added (no panic)
    }
}
