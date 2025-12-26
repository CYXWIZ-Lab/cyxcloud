//! Peer discovery using libp2p with Kademlia DHT
//!
//! Manages the libp2p swarm and provides peer discovery functionality.

use crate::behavior::{BehaviourConfig, CyxCloudBehaviour, CyxCloudEvent};
use futures::StreamExt;
use libp2p::{identity::Keypair, noise, tcp, yamux, Multiaddr, PeerId, Swarm};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Information about a discovered peer
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// libp2p Peer ID
    pub peer_id: PeerId,
    /// Known addresses for this peer
    pub addresses: Vec<Multiaddr>,
    /// gRPC port for ChunkService
    pub grpc_port: u16,
    /// Last time we heard from this peer
    pub last_seen: Instant,
    /// Round-trip time in milliseconds (from ping)
    pub latency_ms: Option<u64>,
    /// Agent version string
    pub agent_version: Option<String>,
}

impl PeerInfo {
    /// Create a new PeerInfo
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            addresses: Vec::new(),
            grpc_port: 50051, // Default gRPC port
            last_seen: Instant::now(),
            latency_ms: None,
            agent_version: None,
        }
    }

    /// Update the last seen timestamp
    pub fn touch(&mut self) {
        self.last_seen = Instant::now();
    }

    /// Check if the peer is stale (not seen for a while)
    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.last_seen.elapsed() > timeout
    }

    /// Get the gRPC address for this peer
    pub fn grpc_address(&self) -> Option<String> {
        // Try to find an IP address
        for addr in &self.addresses {
            let addr_str = addr.to_string();
            if let Some(ip) = extract_ip_from_multiaddr(&addr_str) {
                return Some(format!("{}:{}", ip, self.grpc_port));
            }
        }
        None
    }
}

/// Extract IP address from a multiaddr string
fn extract_ip_from_multiaddr(addr: &str) -> Option<String> {
    // Parse addresses like /ip4/192.168.1.1/udp/4001/quic-v1
    let parts: Vec<&str> = addr.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if (*part == "ip4" || *part == "ip6") && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

/// Configuration for the discovery service
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Listen addresses for libp2p
    pub listen_addrs: Vec<Multiaddr>,
    /// Bootstrap peers to connect to initially
    pub bootstrap_peers: Vec<(PeerId, Multiaddr)>,
    /// gRPC port to advertise
    pub grpc_port: u16,
    /// Peer timeout (remove if not seen for this long)
    pub peer_timeout: Duration,
    /// How often to refresh the peer list
    pub refresh_interval: Duration,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            listen_addrs: vec![
                "/ip4/0.0.0.0/udp/4001/quic-v1".parse().unwrap(),
                "/ip4/0.0.0.0/tcp/4001".parse().unwrap(),
            ],
            bootstrap_peers: Vec::new(),
            grpc_port: 50051,
            peer_timeout: Duration::from_secs(300), // 5 minutes
            refresh_interval: Duration::from_secs(60),
        }
    }
}

impl DiscoveryConfig {
    /// Create a new config with the given listen address
    pub fn new(listen_addr: Multiaddr) -> Self {
        Self {
            listen_addrs: vec![listen_addr],
            ..Default::default()
        }
    }

    /// Add a bootstrap peer
    pub fn with_bootstrap_peer(mut self, peer_id: PeerId, addr: Multiaddr) -> Self {
        self.bootstrap_peers.push((peer_id, addr));
        self
    }

    /// Set the gRPC port
    pub fn with_grpc_port(mut self, port: u16) -> Self {
        self.grpc_port = port;
        self
    }
}

/// Events from the discovery service
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// A new peer was discovered
    PeerDiscovered(PeerInfo),
    /// A peer was removed (expired or disconnected)
    PeerRemoved(PeerId),
    /// Peer latency updated
    PeerLatencyUpdated { peer_id: PeerId, latency_ms: u64 },
}

/// Discovery service that manages libp2p swarm and peer discovery
pub struct DiscoveryService {
    /// libp2p keypair
    keypair: Keypair,
    /// Local peer ID
    local_peer_id: PeerId,
    /// Known peers
    peers: Arc<RwLock<HashMap<PeerId, PeerInfo>>>,
    /// Configuration
    config: DiscoveryConfig,
    /// Event sender (for notifying about peer changes)
    event_tx: Option<mpsc::Sender<DiscoveryEvent>>,
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new(config: DiscoveryConfig) -> Self {
        let keypair = Keypair::generate_ed25519();
        let local_peer_id = keypair.public().to_peer_id();

        info!(peer_id = %local_peer_id, "Created discovery service");

        Self {
            keypair,
            local_peer_id,
            peers: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_tx: None,
        }
    }

    /// Create a new discovery service with the given keypair
    pub fn with_keypair(keypair: Keypair, config: DiscoveryConfig) -> Self {
        let local_peer_id = keypair.public().to_peer_id();

        info!(peer_id = %local_peer_id, "Created discovery service with provided keypair");

        Self {
            keypair,
            local_peer_id,
            peers: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_tx: None,
        }
    }

    /// Get the local peer ID
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Set the event channel for receiving discovery events
    pub fn set_event_channel(&mut self, tx: mpsc::Sender<DiscoveryEvent>) {
        self.event_tx = Some(tx);
    }

    /// Get a list of all known peers
    pub fn get_peers(&self) -> Vec<PeerInfo> {
        let peers = self.peers.read();
        peers.values().cloned().collect()
    }

    /// Get a list of online peers (not stale)
    pub fn get_online_peers(&self) -> Vec<PeerInfo> {
        let peers = self.peers.read();
        peers
            .values()
            .filter(|p| !p.is_stale(self.config.peer_timeout))
            .cloned()
            .collect()
    }

    /// Get a peer by ID
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<PeerInfo> {
        let peers = self.peers.read();
        peers.get(peer_id).cloned()
    }

    /// Get the number of known peers
    pub fn peer_count(&self) -> usize {
        self.peers.read().len()
    }

    /// Build and return the libp2p swarm
    fn build_swarm(&self) -> Result<Swarm<CyxCloudBehaviour>, Box<dyn std::error::Error + Send + Sync>> {
        let keypair = self.keypair.clone();
        let behaviour_config = BehaviourConfig::from_keypair(&keypair);

        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_behaviour(|_key| Ok(CyxCloudBehaviour::new(behaviour_config.clone())))?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        Ok(swarm)
    }

    /// Start the discovery service
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut swarm = self.build_swarm()?;

        // Listen on configured addresses
        for addr in &self.config.listen_addrs {
            match swarm.listen_on(addr.clone()) {
                Ok(_) => info!(addr = %addr, "Listening on address"),
                Err(e) => error!(addr = %addr, error = %e, "Failed to listen on address"),
            }
        }

        // Add bootstrap peers
        for (peer_id, addr) in &self.config.bootstrap_peers {
            swarm.behaviour_mut().add_address(peer_id, addr.clone());
            info!(peer = %peer_id, addr = %addr, "Added bootstrap peer");
        }

        // Start bootstrap if we have peers
        if !self.config.bootstrap_peers.is_empty() {
            match swarm.behaviour_mut().bootstrap() {
                Ok(_) => info!("Started Kademlia bootstrap"),
                Err(e) => warn!(error = ?e, "Bootstrap failed - no known peers"),
            }
        }

        let peers = self.peers.clone();
        let event_tx = self.event_tx.clone();
        let peer_timeout = self.config.peer_timeout;
        let refresh_interval = self.config.refresh_interval;

        // Spawn cleanup task
        let peers_cleanup = peers.clone();
        let event_tx_cleanup = event_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(refresh_interval);
            loop {
                interval.tick().await;

                // Remove stale peers
                let stale: Vec<PeerId> = {
                    let peers = peers_cleanup.read();
                    peers
                        .iter()
                        .filter(|(_, p)| p.is_stale(peer_timeout))
                        .map(|(id, _)| *id)
                        .collect()
                };

                for peer_id in stale {
                    {
                        let mut peers = peers_cleanup.write();
                        peers.remove(&peer_id);
                    }
                    debug!(peer = %peer_id, "Removed stale peer");

                    if let Some(ref tx) = event_tx_cleanup {
                        let _ = tx.send(DiscoveryEvent::PeerRemoved(peer_id)).await;
                    }
                }
            }
        });

        // Main event loop
        loop {
            match swarm.select_next_some().await {
                libp2p::swarm::SwarmEvent::Behaviour(event) => {
                    self.handle_behaviour_event(event, &peers, &event_tx).await;
                }
                libp2p::swarm::SwarmEvent::NewListenAddr { address, .. } => {
                    info!(addr = %address, "New listen address");
                }
                libp2p::swarm::SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    debug!(peer = %peer_id, "Connection established");
                }
                libp2p::swarm::SwarmEvent::ConnectionClosed { peer_id, .. } => {
                    debug!(peer = %peer_id, "Connection closed");
                }
                _ => {}
            }
        }
    }

    async fn handle_behaviour_event(
        &self,
        event: CyxCloudEvent,
        peers: &Arc<RwLock<HashMap<PeerId, PeerInfo>>>,
        event_tx: &Option<mpsc::Sender<DiscoveryEvent>>,
    ) {
        match event {
            CyxCloudEvent::PeerDiscovered { peer_id, addresses } => {
                if peer_id == self.local_peer_id {
                    return; // Skip self
                }

                let mut peers = peers.write();
                let peer = peers.entry(peer_id).or_insert_with(|| PeerInfo::new(peer_id));
                peer.addresses = addresses.clone();
                peer.touch();

                info!(peer = %peer_id, addresses = ?addresses, "Peer discovered");

                if let Some(ref tx) = event_tx {
                    let _ = tx.send(DiscoveryEvent::PeerDiscovered(peer.clone())).await;
                }
            }
            CyxCloudEvent::PeerExpired { peer_id } => {
                let mut peers = peers.write();
                if peers.remove(&peer_id).is_some() {
                    debug!(peer = %peer_id, "Peer expired");

                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(DiscoveryEvent::PeerRemoved(peer_id)).await;
                    }
                }
            }
            CyxCloudEvent::PingResult { peer_id, rtt } => {
                let mut peers = peers.write();
                if let Some(peer) = peers.get_mut(&peer_id) {
                    peer.latency_ms = Some(rtt.as_millis() as u64);
                    peer.touch();

                    if let Some(ref tx) = event_tx {
                        let _ = tx
                            .send(DiscoveryEvent::PeerLatencyUpdated {
                                peer_id,
                                latency_ms: rtt.as_millis() as u64,
                            })
                            .await;
                    }
                }
            }
            CyxCloudEvent::PingFailed { peer_id } => {
                debug!(peer = %peer_id, "Ping failed");
            }
            CyxCloudEvent::IdentifyReceived { peer_id, info } => {
                let mut peers = peers.write();
                let peer = peers.entry(peer_id).or_insert_with(|| PeerInfo::new(peer_id));
                peer.addresses = info.listen_addrs.clone();
                peer.agent_version = Some(info.agent_version);
                peer.touch();

                debug!(peer = %peer_id, "Updated peer info from identify");
            }
            CyxCloudEvent::Kademlia(_) => {
                // Kademlia events are handled internally
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_info() {
        let keypair = Keypair::generate_ed25519();
        let peer_id = keypair.public().to_peer_id();

        let mut info = PeerInfo::new(peer_id);
        assert!(info.last_seen.elapsed() < Duration::from_secs(1));
        assert!(!info.is_stale(Duration::from_secs(60)));

        info.touch();
        assert!(info.last_seen.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_extract_ip() {
        assert_eq!(
            extract_ip_from_multiaddr("/ip4/192.168.1.1/udp/4001/quic-v1"),
            Some("192.168.1.1".to_string())
        );
        assert_eq!(
            extract_ip_from_multiaddr("/ip4/127.0.0.1/tcp/4001"),
            Some("127.0.0.1".to_string())
        );
        assert_eq!(extract_ip_from_multiaddr("/unix/path/to/socket"), None);
    }

    #[test]
    fn test_discovery_config() {
        let config = DiscoveryConfig::default()
            .with_grpc_port(9000);

        assert_eq!(config.grpc_port, 9000);
        assert!(!config.listen_addrs.is_empty());
    }

    #[test]
    fn test_discovery_service_creation() {
        let config = DiscoveryConfig::default();
        let service = DiscoveryService::new(config);

        assert_eq!(service.peer_count(), 0);
        assert!(service.get_peers().is_empty());
    }
}
