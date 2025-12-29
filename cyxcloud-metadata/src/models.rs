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
    pub storage_reserved: i64,  // Gateway-reserved storage (2GB default)
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
    pub storage_reserved: i64,  // Gateway-reserved storage
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
    DataLoss,          // 10% slash
    ExtendedDowntime,  // 5% slash
    CorruptedData,     // 50% slash
    FailedProofs,      // 15% slash
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
