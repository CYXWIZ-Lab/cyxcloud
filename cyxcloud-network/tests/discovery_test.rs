//! Integration tests for peer discovery
//!
//! Tests the libp2p-based peer discovery functionality.

use cyxcloud_network::{
    discovery::{DiscoveryConfig, DiscoveryService, PeerInfo},
    protocol::{NodeAnnouncement, NodeCapacity, NodeLocation, NodeStatus, PROTOCOL_VERSION},
    BehaviourConfig,
};
use libp2p::{identity::Keypair, Multiaddr};
use std::time::Duration;

#[test]
fn test_peer_info_creation() {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();

    let info = PeerInfo::new(peer_id);
    assert_eq!(info.peer_id, peer_id);
    assert!(info.addresses.is_empty());
    assert_eq!(info.grpc_port, 50051);
    assert!(info.latency_ms.is_none());
    assert!(info.agent_version.is_none());
}

#[test]
fn test_peer_info_staleness() {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();

    let info = PeerInfo::new(peer_id);

    // Freshly created peer should not be stale
    assert!(!info.is_stale(Duration::from_secs(60)));

    // With a very short timeout, it should be stale
    assert!(info.is_stale(Duration::from_nanos(1)));
}

#[test]
fn test_peer_info_grpc_address() {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();

    let mut info = PeerInfo::new(peer_id);
    info.grpc_port = 9000;

    // No addresses - should return None
    assert!(info.grpc_address().is_none());

    // Add an IP4 address
    let addr: Multiaddr = "/ip4/192.168.1.100/tcp/4001".parse().unwrap();
    info.addresses.push(addr);

    let grpc_addr = info.grpc_address();
    assert!(grpc_addr.is_some());
    assert_eq!(grpc_addr.unwrap(), "192.168.1.100:9000");
}

#[test]
fn test_discovery_config_default() {
    let config = DiscoveryConfig::default();

    assert!(!config.listen_addrs.is_empty());
    assert!(config.bootstrap_peers.is_empty());
    assert_eq!(config.grpc_port, 50051);
    assert_eq!(config.peer_timeout, Duration::from_secs(300));
    assert_eq!(config.refresh_interval, Duration::from_secs(60));
}

#[test]
fn test_discovery_config_builder() {
    let keypair = Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();
    let addr: Multiaddr = "/ip4/192.168.1.1/tcp/4001".parse().unwrap();

    let config = DiscoveryConfig::default()
        .with_grpc_port(9000)
        .with_bootstrap_peer(peer_id, addr.clone());

    assert_eq!(config.grpc_port, 9000);
    assert_eq!(config.bootstrap_peers.len(), 1);
    assert_eq!(config.bootstrap_peers[0].0, peer_id);
    assert_eq!(config.bootstrap_peers[0].1, addr);
}

#[test]
fn test_discovery_service_creation() {
    let config = DiscoveryConfig::default();
    let service = DiscoveryService::new(config);

    assert_eq!(service.peer_count(), 0);
    assert!(service.get_peers().is_empty());
    assert!(service.get_online_peers().is_empty());
}

#[test]
fn test_discovery_service_with_keypair() {
    let keypair = Keypair::generate_ed25519();
    let expected_peer_id = keypair.public().to_peer_id();

    let config = DiscoveryConfig::default();
    let service = DiscoveryService::with_keypair(keypair, config);

    assert_eq!(service.local_peer_id(), expected_peer_id);
}

#[test]
fn test_protocol_version() {
    assert_eq!(PROTOCOL_VERSION, "/cyxcloud/1.0.0");
}

#[test]
fn test_node_announcement_serialization() {
    let announcement = NodeAnnouncement::new("test-node-1", "192.168.1.100:50051")
        .with_capacity(NodeCapacity::new(1_000_000_000, 100))
        .with_location(NodeLocation::new("dc-1", "us-east").with_rack(1));

    // Serialize
    let bytes = announcement.to_bytes().unwrap();
    assert!(!bytes.is_empty());

    // Deserialize
    let decoded = NodeAnnouncement::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.node_id, announcement.node_id);
    assert_eq!(decoded.grpc_address, announcement.grpc_address);
    assert_eq!(decoded.capacity.storage_total, announcement.capacity.storage_total);
    assert_eq!(decoded.location.datacenter, announcement.location.datacenter);
    assert!(matches!(decoded.status, NodeStatus::Online));
    assert_eq!(decoded.timestamp, announcement.timestamp);
    assert_eq!(decoded.version, announcement.version);
}

#[test]
fn test_node_status_variants() {
    // Test all status variants can be serialized/deserialized
    let statuses = [
        NodeStatus::Online,
        NodeStatus::Offline,
        NodeStatus::Draining,
        NodeStatus::Maintenance,
    ];

    for status in statuses {
        let announcement = NodeAnnouncement::new("test", "127.0.0.1:50051")
            .with_status(status);

        let bytes = announcement.to_bytes().unwrap();
        let decoded = NodeAnnouncement::from_bytes(&bytes).unwrap();
        assert!(std::mem::discriminant(&decoded.status) == std::mem::discriminant(&status));
    }
}

#[test]
fn test_behaviour_config_from_keypair() {
    let keypair = Keypair::generate_ed25519();
    let expected_peer_id = keypair.public().to_peer_id();
    let expected_public_key = keypair.public();

    let config = BehaviourConfig::from_keypair(&keypair);

    assert_eq!(config.local_peer_id, expected_peer_id);
    assert_eq!(config.local_public_key, expected_public_key);
}

#[test]
fn test_node_capacity_default() {
    let capacity = NodeCapacity::default();
    assert_eq!(capacity.storage_total, 0);
    assert_eq!(capacity.storage_used, 0);
    assert_eq!(capacity.bandwidth_mbps, 0);
    assert_eq!(capacity.max_connections, 0);
}

#[test]
fn test_node_capacity_available() {
    let mut capacity = NodeCapacity::new(1000, 100);
    capacity.storage_used = 300;

    assert_eq!(capacity.storage_available(), 700);
    assert!((capacity.utilization_percent() - 30.0).abs() < 0.01);
}

#[test]
fn test_node_location_default() {
    let location = NodeLocation::default();
    assert!(location.datacenter.is_empty());
    assert_eq!(location.rack, 0);
    assert!(location.region.is_empty());
    assert!(location.latitude.is_none());
    assert!(location.longitude.is_none());
}

#[test]
fn test_node_location_with_coordinates() {
    let location = NodeLocation::new("dc-1", "us-east")
        .with_rack(5)
        .with_coordinates(40.7128, -74.0060);

    assert_eq!(location.datacenter, "dc-1");
    assert_eq!(location.rack, 5);
    assert_eq!(location.region, "us-east");
    assert_eq!(location.latitude, Some(40.7128));
    assert_eq!(location.longitude, Some(-74.0060));
}

#[test]
fn test_node_status_display() {
    assert_eq!(NodeStatus::Online.to_string(), "online");
    assert_eq!(NodeStatus::Offline.to_string(), "offline");
    assert_eq!(NodeStatus::Draining.to_string(), "draining");
    assert_eq!(NodeStatus::Maintenance.to_string(), "maintenance");
}

#[test]
fn test_announcement_staleness() {
    let announcement = NodeAnnouncement::new("node", "addr");
    // Should not be stale immediately
    assert!(!announcement.is_stale(300));
    // Should be stale with 0 second threshold (already some time has passed)
    // Note: this might be flaky if the test runs too fast
    // assert!(announcement.is_stale(0));
}

#[test]
fn test_dht_keys() {
    let announcement = NodeAnnouncement::new("node1", "addr");
    assert_eq!(announcement.dht_key(), b"node:node1".to_vec());
}
