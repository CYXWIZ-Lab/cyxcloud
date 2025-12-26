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

    // Metadata
    pub version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Parameters for creating a new node
#[derive(Debug, Clone)]
pub struct CreateNode {
    pub peer_id: String,
    pub grpc_address: String,
    pub storage_total: i64,
    pub bandwidth_mbps: i32,
    pub datacenter: Option<String>,
    pub region: Option<String>,
    pub version: Option<String>,
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
    pub storage_used: i64,
    pub storage_available: i64,
    pub utilization_percent: Option<f64>,
    pub chunk_count: i64,
    pub status: String,
    pub last_heartbeat: Option<DateTime<Utc>>,
}
