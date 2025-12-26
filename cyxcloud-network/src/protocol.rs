//! CyxCloud protocol messages
//!
//! Defines the message formats used for node announcements and metadata exchange
//! over the libp2p DHT.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Protocol version string
pub const PROTOCOL_VERSION: &str = "/cyxcloud/1.0.0";

/// Node announcement message
///
/// This is stored in the Kademlia DHT to advertise a node's presence
/// and capabilities to other nodes in the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAnnouncement {
    /// Unique node identifier (typically the libp2p PeerId string)
    pub node_id: String,
    /// gRPC address for ChunkService (e.g., "192.168.1.10:50051")
    pub grpc_address: String,
    /// Node storage capacity
    pub capacity: NodeCapacity,
    /// Node location information
    pub location: NodeLocation,
    /// Node status
    pub status: NodeStatus,
    /// Unix timestamp when this announcement was created
    pub timestamp: u64,
    /// Protocol version
    pub version: String,
}

impl NodeAnnouncement {
    /// Create a new node announcement
    pub fn new(node_id: impl Into<String>, grpc_address: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            grpc_address: grpc_address.into(),
            capacity: NodeCapacity::default(),
            location: NodeLocation::default(),
            status: NodeStatus::Online,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            version: PROTOCOL_VERSION.to_string(),
        }
    }

    /// Set the capacity
    pub fn with_capacity(mut self, capacity: NodeCapacity) -> Self {
        self.capacity = capacity;
        self
    }

    /// Set the location
    pub fn with_location(mut self, location: NodeLocation) -> Self {
        self.location = location;
        self
    }

    /// Set the status
    pub fn with_status(mut self, status: NodeStatus) -> Self {
        self.status = status;
        self
    }

    /// Serialize to bytes for DHT storage
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Check if the announcement is stale (older than the given duration)
    pub fn is_stale(&self, max_age_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.timestamp) > max_age_secs
    }

    /// Get the DHT key for this node's announcement
    pub fn dht_key(&self) -> Vec<u8> {
        format!("node:{}", self.node_id).into_bytes()
    }
}

/// Node storage capacity
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeCapacity {
    /// Total storage capacity in bytes
    pub storage_total: u64,
    /// Used storage in bytes
    pub storage_used: u64,
    /// Available bandwidth in Mbps
    pub bandwidth_mbps: u32,
    /// Maximum concurrent connections
    pub max_connections: u32,
}

impl NodeCapacity {
    /// Create a new capacity with the given values
    pub fn new(storage_total: u64, bandwidth_mbps: u32) -> Self {
        Self {
            storage_total,
            storage_used: 0,
            bandwidth_mbps,
            max_connections: 100,
        }
    }

    /// Get available storage
    pub fn storage_available(&self) -> u64 {
        self.storage_total.saturating_sub(self.storage_used)
    }

    /// Get storage utilization as a percentage
    pub fn utilization_percent(&self) -> f64 {
        if self.storage_total == 0 {
            0.0
        } else {
            (self.storage_used as f64 / self.storage_total as f64) * 100.0
        }
    }
}

/// Node location information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeLocation {
    /// Datacenter identifier (e.g., "us-east-1")
    pub datacenter: String,
    /// Rack number within datacenter
    pub rack: u32,
    /// Geographic region
    pub region: String,
    /// Latitude (optional)
    pub latitude: Option<f64>,
    /// Longitude (optional)
    pub longitude: Option<f64>,
}

impl NodeLocation {
    /// Create a new location
    pub fn new(datacenter: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            datacenter: datacenter.into(),
            rack: 0,
            region: region.into(),
            latitude: None,
            longitude: None,
        }
    }

    /// Set rack number
    pub fn with_rack(mut self, rack: u32) -> Self {
        self.rack = rack;
        self
    }

    /// Set coordinates
    pub fn with_coordinates(mut self, lat: f64, lon: f64) -> Self {
        self.latitude = Some(lat);
        self.longitude = Some(lon);
        self
    }

    /// Calculate distance to another location (haversine formula)
    pub fn distance_km(&self, other: &NodeLocation) -> Option<f64> {
        match (self.latitude, self.longitude, other.latitude, other.longitude) {
            (Some(lat1), Some(lon1), Some(lat2), Some(lon2)) => {
                let r = 6371.0; // Earth's radius in km
                let lat1_rad = lat1.to_radians();
                let lat2_rad = lat2.to_radians();
                let delta_lat = (lat2 - lat1).to_radians();
                let delta_lon = (lon2 - lon1).to_radians();

                let a = (delta_lat / 2.0).sin().powi(2)
                    + lat1_rad.cos() * lat2_rad.cos() * (delta_lon / 2.0).sin().powi(2);
                let c = 2.0 * a.sqrt().asin();

                Some(r * c)
            }
            _ => None,
        }
    }
}

/// Node status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    /// Node is online and accepting requests
    Online,
    /// Node is offline
    Offline,
    /// Node is draining (not accepting new chunks, migrating data)
    Draining,
    /// Node is in maintenance mode
    Maintenance,
}

impl Default for NodeStatus {
    fn default() -> Self {
        Self::Online
    }
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Draining => write!(f, "draining"),
            Self::Maintenance => write!(f, "maintenance"),
        }
    }
}

/// Chunk location announcement
///
/// Used to announce which chunks a node is storing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkLocationAnnouncement {
    /// Chunk ID (32 bytes)
    pub chunk_id: Vec<u8>,
    /// List of node IDs storing this chunk
    pub node_ids: Vec<String>,
    /// Timestamp of the announcement
    pub timestamp: u64,
}

impl ChunkLocationAnnouncement {
    /// Create a new chunk location announcement
    pub fn new(chunk_id: Vec<u8>, node_ids: Vec<String>) -> Self {
        Self {
            chunk_id,
            node_ids,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Get the DHT key for this chunk
    pub fn dht_key(&self) -> Vec<u8> {
        let mut key = b"chunk:".to_vec();
        key.extend_from_slice(&self.chunk_id);
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_announcement_serialization() {
        let announcement = NodeAnnouncement::new("test-node-1", "192.168.1.10:50051")
            .with_capacity(NodeCapacity::new(1_000_000_000, 1000))
            .with_location(NodeLocation::new("us-east-1", "us-east"));

        let bytes = announcement.to_bytes().unwrap();
        let decoded = NodeAnnouncement::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.node_id, "test-node-1");
        assert_eq!(decoded.grpc_address, "192.168.1.10:50051");
        assert_eq!(decoded.capacity.storage_total, 1_000_000_000);
    }

    #[test]
    fn test_node_capacity() {
        let mut capacity = NodeCapacity::new(1000, 100);
        capacity.storage_used = 250;

        assert_eq!(capacity.storage_available(), 750);
        assert!((capacity.utilization_percent() - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_node_location_distance() {
        // New York to London
        let ny = NodeLocation::new("us-east-1", "us-east")
            .with_coordinates(40.7128, -74.0060);
        let london = NodeLocation::new("eu-west-1", "europe")
            .with_coordinates(51.5074, -0.1278);

        let distance = ny.distance_km(&london).unwrap();
        // Should be approximately 5570 km
        assert!(distance > 5500.0 && distance < 5700.0);
    }

    #[test]
    fn test_node_status_display() {
        assert_eq!(NodeStatus::Online.to_string(), "online");
        assert_eq!(NodeStatus::Draining.to_string(), "draining");
    }

    #[test]
    fn test_announcement_staleness() {
        let announcement = NodeAnnouncement::new("node", "addr");
        // Should not be stale immediately
        assert!(!announcement.is_stale(300));
    }

    #[test]
    fn test_chunk_location_announcement() {
        let chunk_id = vec![0u8; 32];
        let nodes = vec!["node1".to_string(), "node2".to_string()];
        let announcement = ChunkLocationAnnouncement::new(chunk_id.clone(), nodes.clone());

        let bytes = announcement.to_bytes().unwrap();
        let decoded = ChunkLocationAnnouncement::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.chunk_id, chunk_id);
        assert_eq!(decoded.node_ids, nodes);
    }

    #[test]
    fn test_dht_keys() {
        let announcement = NodeAnnouncement::new("node1", "addr");
        assert_eq!(announcement.dht_key(), b"node:node1".to_vec());

        let chunk = ChunkLocationAnnouncement::new(vec![1, 2, 3], vec![]);
        let mut expected_key = b"chunk:".to_vec();
        expected_key.extend_from_slice(&[1, 2, 3]);
        assert_eq!(chunk.dht_key(), expected_key);
    }
}
