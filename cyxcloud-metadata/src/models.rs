//! Database models for CyxCloud metadata
//!
//! These structs map directly to PostgreSQL tables.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Node status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum NodeStatus {
    Online,
    Offline,
    Recovering, // Quarantine period after coming back from offline/draining
    Draining,
    Maintenance,
}

impl Default for NodeStatus {
    fn default() -> Self {
        Self::Offline
    }
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Recovering => write!(f, "recovering"),
            Self::Draining => write!(f, "draining"),
            Self::Maintenance => write!(f, "maintenance"),
        }
    }
}

/// File status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum FileStatus {
    Pending,
    Uploading,
    Complete,
    Failed,
    Deleted,
}

impl Default for FileStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Chunk status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum ChunkStatus {
    Pending,
    Stored,
    Verified,
    Corrupted,
}

impl Default for ChunkStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Location status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum LocationStatus {
    Pending,
    Stored,
    Verified,
    Failed,
}

impl Default for LocationStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Storage node in the network
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub peer_id: String,
    pub grpc_address: String,

    // Capacity
    pub storage_total: i64,
    pub storage_reserved: i64, // Gateway-reserved storage (2GB default)
    pub storage_used: i64,
    pub bandwidth_mbps: i32,
    pub max_connections: i32,

    // Location
    pub datacenter: Option<String>,
    pub rack: Option<i32>,
    pub region: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,

    // Status
    pub status: String,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub failure_count: i32,

    // Fault tolerance tracking
    pub first_offline_at: Option<DateTime<Utc>>,
    pub status_changed_at: Option<DateTime<Utc>>,

    // Metadata
    pub version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    // Identity/Payment
    pub wallet_address: Option<String>,
    pub public_key: Option<String>,
}

impl Node {
    /// Returns the available storage capacity (total - reserved - used)
    pub fn storage_available(&self) -> i64 {
        (self.storage_total - self.storage_reserved - self.storage_used).max(0)
    }

    /// Returns the allocatable storage (total - reserved, for user data)
    pub fn storage_allocatable(&self) -> i64 {
        (self.storage_total - self.storage_reserved).max(0)
    }
}

/// Parameters for creating a new node
#[derive(Debug, Clone)]
pub struct CreateNode {
    pub peer_id: String,
    pub grpc_address: String,
    pub storage_total: i64,
    pub storage_reserved: i64, // Gateway-reserved storage
    pub bandwidth_mbps: i32,
    pub datacenter: Option<String>,
    pub region: Option<String>,
    pub version: Option<String>,
    // Identity/Payment
    pub wallet_address: Option<String>,
    pub public_key: Option<String>,
}

/// File metadata
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct File {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub content_hash: Vec<u8>,

    // Size
    pub size_bytes: i64,
    pub chunk_count: i32,

    // Erasure coding
    pub data_shards: i32,
    pub parity_shards: i32,
    pub chunk_size: i32,

    // Ownership
    pub owner_id: Option<Uuid>,
    pub bucket: Option<String>,

    // Status
    pub status: String,

    // Metadata
    pub content_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Parameters for creating a new file
#[derive(Debug, Clone)]
pub struct CreateFile {
    /// Optional ID - if provided, use this ID; otherwise generate a new one
    pub id: Option<Uuid>,
    pub name: String,
    pub path: String,
    pub content_hash: Vec<u8>,
    pub size_bytes: i64,
    pub chunk_count: i32,
    pub data_shards: i32,
    pub parity_shards: i32,
    pub chunk_size: i32,
    pub owner_id: Option<Uuid>,
    pub bucket: Option<String>,
    pub content_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Chunk metadata
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Chunk {
    pub id: Uuid,
    pub chunk_id: Vec<u8>,
    pub file_id: Uuid,

    // Position
    pub shard_index: i32,
    pub is_parity: bool,

    // Size
    pub size_bytes: i32,

    // Replication
    pub replication_factor: i32,
    pub current_replicas: i32,

    // Status
    pub status: String,

    // Metadata
    pub created_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
}

/// Parameters for creating a new chunk
#[derive(Debug, Clone)]
pub struct CreateChunk {
    pub chunk_id: Vec<u8>,
    pub file_id: Uuid,
    pub shard_index: i32,
    pub is_parity: bool,
    pub size_bytes: i32,
    pub replication_factor: i32,
}

/// Chunk location (which node stores which chunk)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChunkLocation {
    pub id: Uuid,
    pub chunk_id: Vec<u8>,
    pub node_id: Uuid,
    pub status: String,
    pub last_verified: Option<DateTime<Utc>>,
    pub verification_failures: i32,
    pub created_at: DateTime<Utc>,
}

/// User account
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub wallet_address: Option<String>,
    pub email: Option<String>,
    pub username: Option<String>,
    pub storage_quota: i64,
    pub storage_used: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Bucket (S3-compatible)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Bucket {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub versioning_enabled: bool,
    pub public_read: bool,
    pub max_size_bytes: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Repair job for chunk replication
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RepairJob {
    pub id: Uuid,
    pub chunk_id: Vec<u8>,
    pub source_node_id: Option<Uuid>,
    pub target_node_id: Uuid,
    pub status: String,
    pub priority: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub retry_count: i32,
    pub created_at: DateTime<Utc>,
}

/// Chunk replication status view
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChunkReplicationStatus {
    pub chunk_id: Vec<u8>,
    pub file_id: Uuid,
    pub replication_factor: i32,
    pub current_replicas: i32,
    pub replicas_needed: i32,
    pub health_status: String,
}

/// Node storage summary view
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NodeStorageSummary {
    pub id: Uuid,
    pub peer_id: String,
    pub grpc_address: String,
    pub storage_total: i64,
    pub storage_reserved: i64,
    pub storage_used: i64,
    pub storage_available: i64,
    pub storage_allocatable: i64,
    pub utilization_percent: Option<f64>,
    pub chunk_count: i64,
    pub status: String,
    pub last_heartbeat: Option<DateTime<Utc>>,
}

// =============================================================================
// PAYMENT SYSTEM MODELS
// =============================================================================

/// Node uptime tracking per epoch for payment calculation
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct NodeEpochUptime {
    pub id: Uuid,
    pub node_id: Uuid,
    pub epoch: i64,
    pub epoch_start: DateTime<Utc>,
    pub epoch_end: Option<DateTime<Utc>>,
    pub seconds_online: i64,
    pub seconds_offline: i64,
    pub last_status_change: Option<DateTime<Utc>>,
    pub payment_allocated: bool,
    pub payment_amount: Option<i64>,
    pub payment_tx_signature: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl NodeEpochUptime {
    /// Calculate uptime ratio (0.0 to 1.0)
    pub fn uptime_ratio(&self) -> f64 {
        let total = self.seconds_online + self.seconds_offline;
        if total == 0 {
            return 0.0;
        }
        self.seconds_online as f64 / total as f64
    }

    /// Calculate uptime factor for payment (capped at 1.0)
    pub fn uptime_factor(&self, epoch_duration_secs: i64) -> f64 {
        (self.seconds_online as f64 / epoch_duration_secs as f64).min(1.0)
    }
}

/// Slashing reason enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlashReason {
    DataLoss,         // 10% slash
    ExtendedDowntime, // 5% slash
    CorruptedData,    // 50% slash
    FailedProofs,     // 15% slash
}

impl SlashReason {
    /// Get the slash percentage for this reason
    pub fn slash_percent(&self) -> i16 {
        match self {
            Self::DataLoss => 10,
            Self::ExtendedDowntime => 5,
            Self::CorruptedData => 50,
            Self::FailedProofs => 15,
        }
    }

    /// Convert from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "data_loss" => Some(Self::DataLoss),
            "extended_downtime" => Some(Self::ExtendedDowntime),
            "corrupted_data" => Some(Self::CorruptedData),
            "failed_proofs" => Some(Self::FailedProofs),
            _ => None,
        }
    }
}

impl std::fmt::Display for SlashReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DataLoss => write!(f, "data_loss"),
            Self::ExtendedDowntime => write!(f, "extended_downtime"),
            Self::CorruptedData => write!(f, "corrupted_data"),
            Self::FailedProofs => write!(f, "failed_proofs"),
        }
    }
}

/// Slashing event record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SlashingEvent {
    pub id: Uuid,
    pub node_id: Uuid,
    pub epoch: i64,
    pub reason: String,
    pub slash_percent: i16,
    pub slash_amount: Option<i64>,
    pub tx_signature: Option<String>,
    pub details: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Payment epoch state
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PaymentEpoch {
    pub epoch: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub finalized: bool,
    pub total_pool_amount: Option<i64>,
    pub nodes_share: Option<i64>,
    pub platform_share: Option<i64>,
    pub community_share: Option<i64>,
    pub nodes_paid: Option<i32>,
    pub platform_claimed: Option<bool>,
    pub community_claimed: Option<bool>,
    pub finalize_tx_signature: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Node weight for payment calculation
#[derive(Debug, Clone)]
pub struct NodeWeight {
    pub node_id: Uuid,
    pub peer_id: String,
    pub wallet_address: Option<String>,
    pub storage_bytes: i64,
    pub uptime_seconds: i64,
    pub uptime_factor: f64,
    pub reputation: u16,
    pub reputation_factor: f64,
    pub weight: u64,
}

impl NodeWeight {
    /// Calculate node weight from components
    pub fn calculate(
        node_id: Uuid,
        peer_id: String,
        wallet_address: Option<String>,
        storage_bytes: i64,
        uptime_seconds: i64,
        epoch_duration_secs: i64,
        reputation: u16,
    ) -> Self {
        // Uptime factor: 0.0 to 1.0 based on online duration
        let uptime_factor = (uptime_seconds as f64 / epoch_duration_secs as f64).min(1.0);

        // Reputation factor: 0.5 to 1.5 range
        // reputation 0     → 0.5 factor (50% weight reduction)
        // reputation 5000  → 1.0 factor (normal)
        // reputation 10000 → 1.5 factor (50% weight boost)
        let reputation_factor = 0.5 + (reputation as f64 / 10000.0);

        // Final weight = storage × uptime × reputation
        let weight = (storage_bytes as f64 * uptime_factor * reputation_factor) as u64;

        Self {
            node_id,
            peer_id,
            wallet_address,
            storage_bytes,
            uptime_seconds,
            uptime_factor,
            reputation,
            reputation_factor,
            weight,
        }
    }

    /// Calculate this node's share of the total reward pool
    pub fn calculate_share(&self, total_nodes_share: u64, total_weight: u64) -> u64 {
        if total_weight == 0 {
            return 0;
        }
        (total_nodes_share as u128 * self.weight as u128 / total_weight as u128) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPOCH_DURATION: i64 = 7 * 24 * 60 * 60; // 7 days in seconds
    const TB: i64 = 1_000_000_000_000; // 1 TB in bytes

    #[test]
    fn test_node_weight_perfect_node() {
        // Node A: 1TB storage, 100% uptime, perfect reputation (10000)
        let weight = NodeWeight::calculate(
            Uuid::new_v4(),
            "peer_a".to_string(),
            Some("wallet_a".to_string()),
            TB,             // 1 TB storage
            EPOCH_DURATION, // 100% uptime (full week online)
            EPOCH_DURATION,
            10000, // Perfect reputation
        );

        assert_eq!(weight.uptime_factor, 1.0);
        assert_eq!(weight.reputation_factor, 1.5); // 0.5 + (10000/10000)
                                                   // weight = 1TB * 1.0 * 1.5 = 1.5TB
        assert_eq!(weight.weight, (TB as f64 * 1.5) as u64);
        println!("Perfect node weight: {}", weight.weight);
    }

    #[test]
    fn test_node_weight_average_node() {
        // Node B: 500GB storage, 50% uptime, average reputation (5000)
        let weight = NodeWeight::calculate(
            Uuid::new_v4(),
            "peer_b".to_string(),
            Some("wallet_b".to_string()),
            TB / 2,             // 500 GB storage
            EPOCH_DURATION / 2, // 50% uptime (3.5 days online)
            EPOCH_DURATION,
            5000, // Average reputation
        );

        assert_eq!(weight.uptime_factor, 0.5);
        assert_eq!(weight.reputation_factor, 1.0); // 0.5 + (5000/10000)
                                                   // weight = 500GB * 0.5 * 1.0 = 250GB
        assert_eq!(weight.weight, (TB as f64 / 2.0 * 0.5 * 1.0) as u64);
        println!("Average node weight: {}", weight.weight);
    }

    #[test]
    fn test_node_weight_poor_reputation() {
        // Node with poor reputation (0) - gets 50% weight penalty
        let weight = NodeWeight::calculate(
            Uuid::new_v4(),
            "peer_c".to_string(),
            None,
            TB,
            EPOCH_DURATION,
            EPOCH_DURATION,
            0, // Zero reputation
        );

        assert_eq!(weight.reputation_factor, 0.5); // 0.5 + (0/10000)
                                                   // weight = 1TB * 1.0 * 0.5 = 0.5TB
        assert_eq!(weight.weight, (TB as f64 * 0.5) as u64);
        println!("Poor reputation node weight: {}", weight.weight);
    }

    #[test]
    fn test_payment_distribution() {
        // Simulate payment distribution from docs/payment_system.md example
        let node_a = NodeWeight::calculate(
            Uuid::new_v4(),
            "node_a".to_string(),
            Some("wallet_a".to_string()),
            TB,             // 1 TB
            EPOCH_DURATION, // 100% uptime
            EPOCH_DURATION,
            10000, // Perfect reputation
        );

        let node_b = NodeWeight::calculate(
            Uuid::new_v4(),
            "node_b".to_string(),
            Some("wallet_b".to_string()),
            TB / 2,             // 500 GB
            EPOCH_DURATION / 2, // 50% uptime
            EPOCH_DURATION,
            5000, // Average reputation
        );

        let total_weight = node_a.weight + node_b.weight;
        let nodes_share = 850_000_000_000u64; // 850 CYXWIZ (85% of 1000)

        let reward_a = node_a.calculate_share(nodes_share, total_weight);
        let reward_b = node_b.calculate_share(nodes_share, total_weight);

        println!("\n=== Payment Distribution Test ===");
        println!("Node A: {} weight, {} reward", node_a.weight, reward_a);
        println!("Node B: {} weight, {} reward", node_b.weight, reward_b);
        println!("Total weight: {}", total_weight);
        println!("Total distributed: {}", reward_a + reward_b);

        // Node A should get ~85.7% (6/7) of the rewards
        // Node B should get ~14.3% (1/7) of the rewards
        assert!(reward_a > reward_b * 5); // A should get at least 5x more than B
                                          // Allow for small rounding error (integer division)
        let total_distributed = reward_a + reward_b;
        assert!(total_distributed >= nodes_share - 1 && total_distributed <= nodes_share);
    }

    #[test]
    fn test_reputation_impact() {
        // Same storage and uptime, different reputation
        let low_rep = NodeWeight::calculate(
            Uuid::new_v4(),
            "low".to_string(),
            None,
            TB,
            EPOCH_DURATION,
            EPOCH_DURATION,
            0,
        );

        let mid_rep = NodeWeight::calculate(
            Uuid::new_v4(),
            "mid".to_string(),
            None,
            TB,
            EPOCH_DURATION,
            EPOCH_DURATION,
            5000,
        );

        let high_rep = NodeWeight::calculate(
            Uuid::new_v4(),
            "high".to_string(),
            None,
            TB,
            EPOCH_DURATION,
            EPOCH_DURATION,
            10000,
        );

        println!("\n=== Reputation Impact Test ===");
        println!(
            "Low rep (0):     factor={:.2}, weight={}",
            low_rep.reputation_factor, low_rep.weight
        );
        println!(
            "Mid rep (5000):  factor={:.2}, weight={}",
            mid_rep.reputation_factor, mid_rep.weight
        );
        println!(
            "High rep (10000): factor={:.2}, weight={}",
            high_rep.reputation_factor, high_rep.weight
        );

        // Reputation factors should be 0.5, 1.0, 1.5
        assert_eq!(low_rep.reputation_factor, 0.5);
        assert_eq!(mid_rep.reputation_factor, 1.0);
        assert_eq!(high_rep.reputation_factor, 1.5);

        // High rep node gets 3x weight of low rep node
        assert_eq!(high_rep.weight, low_rep.weight * 3);
    }

    #[test]
    fn test_uptime_epoch_tracking() {
        let uptime = NodeEpochUptime {
            id: Uuid::new_v4(),
            node_id: Uuid::new_v4(),
            epoch: 1,
            epoch_start: chrono::Utc::now(),
            epoch_end: None,
            seconds_online: 302400,  // 3.5 days
            seconds_offline: 302400, // 3.5 days
            last_status_change: None,
            payment_allocated: false,
            payment_amount: None,
            payment_tx_signature: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert_eq!(uptime.uptime_ratio(), 0.5);
        assert_eq!(uptime.uptime_factor(EPOCH_DURATION), 0.5);
    }

    #[test]
    fn test_slash_reasons() {
        assert_eq!(SlashReason::ExtendedDowntime.slash_percent(), 5);
        assert_eq!(SlashReason::DataLoss.slash_percent(), 10);
        assert_eq!(SlashReason::FailedProofs.slash_percent(), 15);
        assert_eq!(SlashReason::CorruptedData.slash_percent(), 50);

        assert_eq!(
            SlashReason::from_str("extended_downtime"),
            Some(SlashReason::ExtendedDowntime)
        );
        assert_eq!(
            SlashReason::from_str("data_loss"),
            Some(SlashReason::DataLoss)
        );
        assert_eq!(SlashReason::from_str("invalid"), None);
    }
}
