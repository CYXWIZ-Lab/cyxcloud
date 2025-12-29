//! PostgreSQL database operations for CyxCloud metadata
//!
//! Provides CRUD operations and queries using SQLx.

use crate::models::*;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, instrument};
use uuid::Uuid;

/// Database error types
#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Record not found: {0}")]
    NotFound(String),

    #[error("Duplicate entry: {0}")]
    Duplicate(String),

    #[error("Invalid data: {0}")]
    Invalid(String),

    #[error("Migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Database configuration
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
}

/// Fault tolerance threshold configuration
#[derive(Debug, Clone)]
pub struct FaultToleranceConfig {
    /// Mark node offline after no heartbeat for this duration (default: 5 minutes)
    pub offline_threshold: Duration,
    /// Start draining node after offline for this duration (default: 4 hours)
    pub drain_threshold: Duration,
    /// Remove node from database after offline for this duration (default: 7 days)
    pub remove_threshold: Duration,
    /// Recovery quarantine period before marking fully online (default: 5 minutes)
    pub recovery_quarantine: Duration,
}

impl Default for FaultToleranceConfig {
    fn default() -> Self {
        Self {
            offline_threshold: Duration::from_secs(5 * 60),         // 5 minutes
            drain_threshold: Duration::from_secs(4 * 60 * 60),      // 4 hours
            remove_threshold: Duration::from_secs(7 * 24 * 60 * 60), // 7 days
            recovery_quarantine: Duration::from_secs(5 * 60),       // 5 minutes
        }
    }
}

impl FaultToleranceConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            offline_threshold: Duration::from_secs(
                std::env::var("NODE_OFFLINE_THRESHOLD_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5 * 60),
            ),
            drain_threshold: Duration::from_secs(
                std::env::var("NODE_DRAIN_THRESHOLD_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(4 * 60 * 60),
            ),
            remove_threshold: Duration::from_secs(
                std::env::var("NODE_REMOVE_THRESHOLD_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(7 * 24 * 60 * 60),
            ),
            recovery_quarantine: Duration::from_secs(
                std::env::var("NODE_RECOVERY_QUARANTINE_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5 * 60),
            ),
        }
    }
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            url: "postgres://localhost/cyxcloud".to_string(),
            max_connections: 10,
            min_connections: 2,
            connect_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

/// PostgreSQL database client
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database connection pool
    pub async fn new(config: DbConfig) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(config.connect_timeout)
            .idle_timeout(config.idle_timeout)
            .connect(&config.url)
            .await?;

        info!("Connected to PostgreSQL database");
        Ok(Self { pool })
    }

    /// Run migrations
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        info!("Database migrations complete");
        Ok(())
    }

    /// Get the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // =========================================================================
    // NODE OPERATIONS
    // =========================================================================

    /// Register a new node (upsert - updates if peer_id exists)
    #[instrument(skip(self, node))]
    pub async fn create_node(&self, node: CreateNode) -> Result<Node> {
        let result = sqlx::query_as::<_, Node>(
            r#"
            INSERT INTO nodes (peer_id, grpc_address, storage_total, storage_reserved, bandwidth_mbps, datacenter, region, version, wallet_address, public_key, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'online')
            ON CONFLICT (peer_id) DO UPDATE SET
                grpc_address = EXCLUDED.grpc_address,
                storage_total = EXCLUDED.storage_total,
                storage_reserved = EXCLUDED.storage_reserved,
                bandwidth_mbps = EXCLUDED.bandwidth_mbps,
                datacenter = EXCLUDED.datacenter,
                region = EXCLUDED.region,
                version = EXCLUDED.version,
                wallet_address = COALESCE(EXCLUDED.wallet_address, nodes.wallet_address),
                public_key = COALESCE(EXCLUDED.public_key, nodes.public_key),
                status = 'online',
                last_heartbeat = NOW(),
                first_offline_at = NULL
            RETURNING *
            "#,
        )
        .bind(&node.peer_id)
        .bind(&node.grpc_address)
        .bind(node.storage_total)
        .bind(node.storage_reserved)
        .bind(node.bandwidth_mbps)
        .bind(&node.datacenter)
        .bind(&node.region)
        .bind(&node.version)
        .bind(&node.wallet_address)
        .bind(&node.public_key)
        .fetch_one(&self.pool)
        .await?;

        debug!(node_id = %result.id, peer_id = %node.peer_id, "Node registered/updated");
        Ok(result)
    }

    /// Get a node by ID
    pub async fn get_node(&self, id: Uuid) -> Result<Option<Node>> {
        let result = sqlx::query_as::<_, Node>("SELECT * FROM nodes WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(result)
    }

    /// Get a node by peer ID
    pub async fn get_node_by_peer_id(&self, peer_id: &str) -> Result<Option<Node>> {
        let result = sqlx::query_as::<_, Node>("SELECT * FROM nodes WHERE peer_id = $1")
            .bind(peer_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(result)
    }

    /// Get all online nodes
    pub async fn get_online_nodes(&self) -> Result<Vec<Node>> {
        let result = sqlx::query_as::<_, Node>(
            "SELECT * FROM nodes WHERE status = 'online' ORDER BY storage_used ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get nodes by region
    pub async fn get_nodes_by_region(&self, region: &str) -> Result<Vec<Node>> {
        let result = sqlx::query_as::<_, Node>(
            "SELECT * FROM nodes WHERE region = $1 AND status = 'online'",
        )
        .bind(region)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update node heartbeat
    #[instrument(skip(self))]
    pub async fn update_node_heartbeat(&self, node_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE nodes
            SET last_heartbeat = NOW(), status = 'online', failure_count = 0
            WHERE id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update node status
    pub async fn update_node_status(&self, node_id: Uuid, status: &str) -> Result<()> {
        sqlx::query("UPDATE nodes SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update node storage usage
    pub async fn update_node_storage(&self, node_id: Uuid, storage_used: i64) -> Result<()> {
        sqlx::query("UPDATE nodes SET storage_used = $1 WHERE id = $2")
            .bind(storage_used)
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Increment node failure count
    pub async fn increment_node_failures(&self, node_id: Uuid) -> Result<i32> {
        let result = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE nodes
            SET failure_count = failure_count + 1
            WHERE id = $1
            RETURNING failure_count
            "#,
        )
        .bind(node_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get node storage summary
    pub async fn get_node_storage_summary(&self) -> Result<Vec<NodeStorageSummary>> {
        let result =
            sqlx::query_as::<_, NodeStorageSummary>("SELECT * FROM node_storage_summary")
                .fetch_all(&self.pool)
                .await?;
        Ok(result)
    }

    // =========================================================================
    // FAULT TOLERANCE OPERATIONS
    // =========================================================================

    /// Get online nodes that have not sent a heartbeat within the threshold
    /// These should be marked as offline
    #[instrument(skip(self))]
    pub async fn get_stale_online_nodes(&self, threshold: Duration) -> Result<Vec<Node>> {
        let threshold_secs = threshold.as_secs() as i64;
        let result = sqlx::query_as::<_, Node>(
            r#"
            SELECT * FROM nodes
            WHERE status = 'online'
            AND (last_heartbeat IS NULL OR last_heartbeat < NOW() - make_interval(secs => $1))
            "#,
        )
        .bind(threshold_secs)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get offline nodes that should start draining (offline > threshold)
    #[instrument(skip(self))]
    pub async fn get_nodes_for_draining(&self, threshold: Duration) -> Result<Vec<Node>> {
        let threshold_secs = threshold.as_secs() as i64;
        let result = sqlx::query_as::<_, Node>(
            r#"
            SELECT * FROM nodes
            WHERE status = 'offline'
            AND first_offline_at IS NOT NULL
            AND first_offline_at < NOW() - make_interval(secs => $1)
            "#,
        )
        .bind(threshold_secs)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get nodes that should be removed (offline/draining > threshold)
    #[instrument(skip(self))]
    pub async fn get_nodes_for_removal(&self, threshold: Duration) -> Result<Vec<Node>> {
        let threshold_secs = threshold.as_secs() as i64;
        let result = sqlx::query_as::<_, Node>(
            r#"
            SELECT * FROM nodes
            WHERE status IN ('offline', 'draining')
            AND first_offline_at IS NOT NULL
            AND first_offline_at < NOW() - make_interval(secs => $1)
            "#,
        )
        .bind(threshold_secs)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get recovering nodes that have completed their quarantine period
    #[instrument(skip(self))]
    pub async fn get_recovered_nodes(&self, quarantine: Duration) -> Result<Vec<Node>> {
        let quarantine_secs = quarantine.as_secs() as i64;
        let result = sqlx::query_as::<_, Node>(
            r#"
            SELECT * FROM nodes
            WHERE status = 'recovering'
            AND status_changed_at IS NOT NULL
            AND status_changed_at < NOW() - make_interval(secs => $1)
            "#,
        )
        .bind(quarantine_secs)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Mark a node as offline and record when it first went offline
    #[instrument(skip(self))]
    pub async fn mark_node_offline(&self, node_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE nodes
            SET status = 'offline',
                status_changed_at = NOW(),
                first_offline_at = COALESCE(first_offline_at, NOW())
            WHERE id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        debug!(node_id = %node_id, "Node marked as offline");
        Ok(())
    }

    /// Mark a node as recovering (quarantine state after coming back)
    #[instrument(skip(self))]
    pub async fn mark_node_recovering(&self, node_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE nodes
            SET status = 'recovering',
                status_changed_at = NOW(),
                last_heartbeat = NOW(),
                failure_count = 0
            WHERE id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        debug!(node_id = %node_id, "Node marked as recovering");
        Ok(())
    }

    /// Mark a recovering node as fully online (quarantine complete)
    #[instrument(skip(self))]
    pub async fn mark_node_online(&self, node_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE nodes
            SET status = 'online',
                status_changed_at = NOW(),
                first_offline_at = NULL,
                last_heartbeat = NOW(),
                failure_count = 0
            WHERE id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        debug!(node_id = %node_id, "Node marked as online");
        Ok(())
    }

    /// Mark a node as draining (chunk evacuation should begin)
    #[instrument(skip(self))]
    pub async fn mark_node_draining(&self, node_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE nodes
            SET status = 'draining',
                status_changed_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        debug!(node_id = %node_id, "Node marked as draining");
        Ok(())
    }

    /// Update node heartbeat with recovery-aware logic
    /// Returns the new status after the update
    #[instrument(skip(self))]
    pub async fn update_node_heartbeat_with_recovery(&self, node_id: Uuid) -> Result<NodeStatus> {
        // First get the current status
        let node = self.get_node(node_id).await?
            .ok_or_else(|| DbError::NotFound(format!("Node {} not found", node_id)))?;

        match node.status.as_str() {
            "online" | "recovering" => {
                // Just update heartbeat timestamp
                sqlx::query(
                    r#"
                    UPDATE nodes
                    SET last_heartbeat = NOW(), failure_count = 0
                    WHERE id = $1
                    "#,
                )
                .bind(node_id)
                .execute(&self.pool)
                .await?;

                if node.status == "recovering" {
                    debug!(node_id = %node_id, "Heartbeat updated (recovering)");
                    Ok(NodeStatus::Recovering)
                } else {
                    debug!(node_id = %node_id, "Heartbeat updated (online)");
                    Ok(NodeStatus::Online)
                }
            }
            "offline" | "draining" => {
                // Node coming back - enter quarantine
                sqlx::query(
                    r#"
                    UPDATE nodes
                    SET status = 'recovering',
                        status_changed_at = NOW(),
                        last_heartbeat = NOW(),
                        failure_count = 0
                    WHERE id = $1
                    "#,
                )
                .bind(node_id)
                .execute(&self.pool)
                .await?;

                debug!(node_id = %node_id, "Node entering recovery quarantine");
                Ok(NodeStatus::Recovering)
            }
            _ => {
                // maintenance or unknown - just update heartbeat
                sqlx::query("UPDATE nodes SET last_heartbeat = NOW() WHERE id = $1")
                    .bind(node_id)
                    .execute(&self.pool)
                    .await?;

                debug!(node_id = %node_id, status = %node.status, "Heartbeat updated");
                Ok(NodeStatus::Maintenance)
            }
        }
    }

    /// Update node heartbeat using peer_id (client-side node identifier)
    /// This is used by heartbeat RPCs where the node sends its own ID
    #[instrument(skip(self))]
    pub async fn update_node_heartbeat_by_peer_id(&self, peer_id: &str) -> Result<NodeStatus> {
        // First get the node by peer_id
        let node = self.get_node_by_peer_id(peer_id).await?
            .ok_or_else(|| DbError::NotFound(format!("Node with peer_id {} not found", peer_id)))?;

        match node.status.as_str() {
            "online" | "recovering" => {
                // Just update heartbeat timestamp
                sqlx::query(
                    r#"
                    UPDATE nodes
                    SET last_heartbeat = NOW(), failure_count = 0
                    WHERE peer_id = $1
                    "#,
                )
                .bind(peer_id)
                .execute(&self.pool)
                .await?;

                if node.status == "recovering" {
                    debug!(peer_id = %peer_id, "Heartbeat updated (recovering)");
                    Ok(NodeStatus::Recovering)
                } else {
                    debug!(peer_id = %peer_id, "Heartbeat updated (online)");
                    Ok(NodeStatus::Online)
                }
            }
            "offline" | "draining" => {
                // Node coming back - enter quarantine
                sqlx::query(
                    r#"
                    UPDATE nodes
                    SET status = 'recovering',
                        status_changed_at = NOW(),
                        last_heartbeat = NOW(),
                        failure_count = 0
                    WHERE peer_id = $1
                    "#,
                )
                .bind(peer_id)
                .execute(&self.pool)
                .await?;

                debug!(peer_id = %peer_id, "Node entering recovery quarantine");
                Ok(NodeStatus::Recovering)
            }
            _ => {
                // maintenance or unknown - just update heartbeat
                sqlx::query("UPDATE nodes SET last_heartbeat = NOW() WHERE peer_id = $1")
                    .bind(peer_id)
                    .execute(&self.pool)
                    .await?;

                debug!(peer_id = %peer_id, status = %node.status, "Heartbeat updated");
                Ok(NodeStatus::Maintenance)
            }
        }
    }

    /// Get all chunk locations stored on a specific node
    pub async fn get_chunks_on_node(&self, node_id: Uuid) -> Result<Vec<ChunkLocation>> {
        let result = sqlx::query_as::<_, ChunkLocation>(
            "SELECT * FROM chunk_locations WHERE node_id = $1 AND status = 'stored'",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Delete a node from the database
    /// Note: chunk_locations has ON DELETE CASCADE, so locations are auto-deleted
    #[instrument(skip(self))]
    pub async fn delete_node(&self, node_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM nodes WHERE id = $1")
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        debug!(node_id = %node_id, "Node deleted from database");
        Ok(())
    }

    /// Get all nodes (for health monitoring)
    pub async fn get_all_nodes(&self) -> Result<Vec<Node>> {
        let result = sqlx::query_as::<_, Node>("SELECT * FROM nodes ORDER BY created_at")
            .fetch_all(&self.pool)
            .await?;
        Ok(result)
    }

    // =========================================================================
    // FILE OPERATIONS
    // =========================================================================

    /// Create a new file record
    #[instrument(skip(self, file))]
    pub async fn create_file(&self, file: CreateFile) -> Result<File> {
        // Use provided ID or generate a new one
        let file_id = file.id.unwrap_or_else(Uuid::new_v4);

        let result = sqlx::query_as::<_, File>(
            r#"
            INSERT INTO files (id, name, path, content_hash, size_bytes, chunk_count,
                              data_shards, parity_shards, chunk_size, owner_id, bucket,
                              content_type, metadata, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'pending')
            RETURNING *
            "#,
        )
        .bind(file_id)
        .bind(&file.name)
        .bind(&file.path)
        .bind(&file.content_hash)
        .bind(file.size_bytes)
        .bind(file.chunk_count)
        .bind(file.data_shards)
        .bind(file.parity_shards)
        .bind(file.chunk_size)
        .bind(file.owner_id)
        .bind(&file.bucket)
        .bind(&file.content_type)
        .bind(&file.metadata)
        .fetch_one(&self.pool)
        .await?;

        debug!(file_id = %result.id, path = %file.path, "File created");
        Ok(result)
    }

    /// Get a file by ID
    pub async fn get_file(&self, id: Uuid) -> Result<Option<File>> {
        let result = sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get a file by path
    pub async fn get_file_by_path(&self, path: &str) -> Result<Option<File>> {
        let result = sqlx::query_as::<_, File>(
            "SELECT * FROM files WHERE path = $1 AND deleted_at IS NULL",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// List files in a bucket
    pub async fn list_files_in_bucket(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<File>> {
        let result = if let Some(prefix) = prefix {
            sqlx::query_as::<_, File>(
                r#"
                SELECT * FROM files
                WHERE bucket = $1 AND path LIKE $2 AND deleted_at IS NULL
                ORDER BY path
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(bucket)
            .bind(format!("{}%", prefix))
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, File>(
                r#"
                SELECT * FROM files
                WHERE bucket = $1 AND deleted_at IS NULL
                ORDER BY path
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(bucket)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(result)
    }

    /// Update file status
    pub async fn update_file_status(&self, file_id: Uuid, status: &str) -> Result<()> {
        sqlx::query("UPDATE files SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(file_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Soft delete a file
    pub async fn delete_file(&self, file_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE files SET deleted_at = NOW(), status = 'deleted' WHERE id = $1")
            .bind(file_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // =========================================================================
    // CHUNK OPERATIONS
    // =========================================================================

    /// Create a new chunk record
    #[instrument(skip(self, chunk))]
    pub async fn create_chunk(&self, chunk: CreateChunk) -> Result<Chunk> {
        let result = sqlx::query_as::<_, Chunk>(
            r#"
            INSERT INTO chunks (chunk_id, file_id, shard_index, is_parity, size_bytes, replication_factor)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(&chunk.chunk_id)
        .bind(chunk.file_id)
        .bind(chunk.shard_index)
        .bind(chunk.is_parity)
        .bind(chunk.size_bytes)
        .bind(chunk.replication_factor)
        .fetch_one(&self.pool)
        .await?;

        debug!(chunk_id = ?chunk.chunk_id, file_id = %chunk.file_id, "Chunk created");
        Ok(result)
    }

    /// Get a chunk by chunk_id
    pub async fn get_chunk_by_id(&self, chunk_id: &[u8]) -> Result<Option<Chunk>> {
        let result = sqlx::query_as::<_, Chunk>("SELECT * FROM chunks WHERE chunk_id = $1")
            .bind(chunk_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(result)
    }

    /// Get all chunks for a file
    pub async fn get_file_chunks(&self, file_id: Uuid) -> Result<Vec<Chunk>> {
        let result = sqlx::query_as::<_, Chunk>(
            "SELECT * FROM chunks WHERE file_id = $1 ORDER BY shard_index",
        )
        .bind(file_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update chunk status
    pub async fn update_chunk_status(&self, chunk_id: &[u8], status: &str) -> Result<()> {
        sqlx::query("UPDATE chunks SET status = $1 WHERE chunk_id = $2")
            .bind(status)
            .bind(chunk_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update chunk replica count
    pub async fn update_chunk_replicas(&self, chunk_id: &[u8], count: i32) -> Result<()> {
        sqlx::query("UPDATE chunks SET current_replicas = $1 WHERE chunk_id = $2")
            .bind(count)
            .bind(chunk_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Increment chunk replica count
    pub async fn increment_chunk_replicas(&self, chunk_id: &[u8]) -> Result<i32> {
        let result = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE chunks
            SET current_replicas = current_replicas + 1
            WHERE chunk_id = $1
            RETURNING current_replicas
            "#,
        )
        .bind(chunk_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get under-replicated chunks
    pub async fn get_under_replicated_chunks(&self, limit: i64) -> Result<Vec<ChunkReplicationStatus>> {
        let result = sqlx::query_as::<_, ChunkReplicationStatus>(
            r#"
            SELECT * FROM chunk_replication_status
            WHERE replicas_needed > 0
            ORDER BY replicas_needed DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    // =========================================================================
    // CHUNK LOCATION OPERATIONS
    // =========================================================================

    /// Record a chunk location
    pub async fn add_chunk_location(
        &self,
        chunk_id: &[u8],
        node_id: Uuid,
    ) -> Result<ChunkLocation> {
        let result = sqlx::query_as::<_, ChunkLocation>(
            r#"
            INSERT INTO chunk_locations (chunk_id, node_id, status)
            VALUES ($1, $2, 'stored')
            ON CONFLICT (chunk_id, node_id) DO UPDATE SET status = 'stored'
            RETURNING *
            "#,
        )
        .bind(chunk_id)
        .bind(node_id)
        .fetch_one(&self.pool)
        .await?;

        // Update chunk replica count
        self.increment_chunk_replicas(chunk_id).await?;

        Ok(result)
    }

    /// Get all locations for a chunk
    pub async fn get_chunk_locations(&self, chunk_id: &[u8]) -> Result<Vec<ChunkLocation>> {
        let result = sqlx::query_as::<_, ChunkLocation>(
            "SELECT * FROM chunk_locations WHERE chunk_id = $1 AND status = 'stored'",
        )
        .bind(chunk_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get node addresses storing a chunk
    pub async fn get_chunk_node_addresses(&self, chunk_id: &[u8]) -> Result<Vec<String>> {
        let result = sqlx::query_scalar::<_, String>(
            r#"
            SELECT n.grpc_address
            FROM chunk_locations cl
            JOIN nodes n ON cl.node_id = n.id
            WHERE cl.chunk_id = $1 AND cl.status = 'stored' AND n.status = 'online'
            "#,
        )
        .bind(chunk_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Remove a chunk location (e.g., node went offline)
    pub async fn remove_chunk_location(&self, chunk_id: &[u8], node_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM chunk_locations WHERE chunk_id = $1 AND node_id = $2")
            .bind(chunk_id)
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update chunk location verification
    pub async fn update_chunk_verification(
        &self,
        chunk_id: &[u8],
        node_id: Uuid,
        valid: bool,
    ) -> Result<()> {
        if valid {
            sqlx::query(
                r#"
                UPDATE chunk_locations
                SET last_verified = NOW(), status = 'verified', verification_failures = 0
                WHERE chunk_id = $1 AND node_id = $2
                "#,
            )
            .bind(chunk_id)
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE chunk_locations
                SET verification_failures = verification_failures + 1,
                    status = CASE WHEN verification_failures >= 2 THEN 'failed' ELSE status END
                WHERE chunk_id = $1 AND node_id = $2
                "#,
            )
            .bind(chunk_id)
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    // =========================================================================
    // USER OPERATIONS
    // =========================================================================

    /// Create a new user
    pub async fn create_user(
        &self,
        wallet_address: Option<&str>,
        email: Option<&str>,
        username: Option<&str>,
    ) -> Result<User> {
        let result = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (wallet_address, email, username)
            VALUES ($1, $2, $3)
            RETURNING *
            "#,
        )
        .bind(wallet_address)
        .bind(email)
        .bind(username)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get user by wallet address
    pub async fn get_user_by_wallet(&self, wallet_address: &str) -> Result<Option<User>> {
        let result = sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE wallet_address = $1",
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update user storage usage
    pub async fn update_user_storage(&self, user_id: Uuid, storage_used: i64) -> Result<()> {
        sqlx::query("UPDATE users SET storage_used = $1 WHERE id = $2")
            .bind(storage_used)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // =========================================================================
    // BUCKET OPERATIONS
    // =========================================================================

    /// Create a new bucket
    pub async fn create_bucket(&self, name: &str, owner_id: Uuid) -> Result<Bucket> {
        let result = sqlx::query_as::<_, Bucket>(
            r#"
            INSERT INTO buckets (name, owner_id)
            VALUES ($1, $2)
            RETURNING *
            "#,
        )
        .bind(name)
        .bind(owner_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get bucket by name
    pub async fn get_bucket(&self, name: &str) -> Result<Option<Bucket>> {
        let result = sqlx::query_as::<_, Bucket>("SELECT * FROM buckets WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(result)
    }

    /// List buckets for a user
    pub async fn list_user_buckets(&self, owner_id: Uuid) -> Result<Vec<Bucket>> {
        let result = sqlx::query_as::<_, Bucket>(
            "SELECT * FROM buckets WHERE owner_id = $1 ORDER BY name",
        )
        .bind(owner_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Delete a bucket by name
    ///
    /// Note: This performs a hard delete. Make sure the bucket is empty first.
    pub async fn delete_bucket(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM buckets WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Check if a bucket is empty (has no files)
    pub async fn bucket_is_empty(&self, bucket_name: &str) -> Result<bool> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM files WHERE bucket = $1 AND deleted_at IS NULL",
        )
        .bind(bucket_name)
        .fetch_one(&self.pool)
        .await?;
        Ok(count.0 == 0)
    }

    /// Count files in a bucket
    pub async fn count_files_in_bucket(&self, bucket_name: &str) -> Result<i64> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM files WHERE bucket = $1 AND deleted_at IS NULL",
        )
        .bind(bucket_name)
        .fetch_one(&self.pool)
        .await?;
        Ok(count.0)
    }

    // =========================================================================
    // REPAIR JOB OPERATIONS
    // =========================================================================

    /// Create a repair job
    pub async fn create_repair_job(
        &self,
        chunk_id: &[u8],
        source_node_id: Option<Uuid>,
        target_node_id: Uuid,
        priority: i32,
    ) -> Result<RepairJob> {
        let result = sqlx::query_as::<_, RepairJob>(
            r#"
            INSERT INTO repair_jobs (chunk_id, source_node_id, target_node_id, priority)
            VALUES ($1, $2, $3, $4)
            RETURNING *
            "#,
        )
        .bind(chunk_id)
        .bind(source_node_id)
        .bind(target_node_id)
        .bind(priority)
        .fetch_one(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get pending repair jobs
    pub async fn get_pending_repair_jobs(&self, limit: i64) -> Result<Vec<RepairJob>> {
        let result = sqlx::query_as::<_, RepairJob>(
            r#"
            SELECT * FROM repair_jobs
            WHERE status = 'pending'
            ORDER BY priority DESC, created_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update repair job status
    pub async fn update_repair_job_status(
        &self,
        job_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE repair_jobs
            SET status = $1,
                error_message = $2,
                started_at = CASE WHEN $1 = 'in_progress' THEN NOW() ELSE started_at END,
                completed_at = CASE WHEN $1 IN ('completed', 'failed') THEN NOW() ELSE completed_at END
            WHERE id = $3
            "#,
        )
        .bind(status)
        .bind(error)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_config_default() {
        let config = DbConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.min_connections, 2);
    }

    #[test]
    fn test_node_status_display() {
        assert_eq!(NodeStatus::Online.to_string(), "online");
        assert_eq!(NodeStatus::Offline.to_string(), "offline");
        assert_eq!(NodeStatus::Recovering.to_string(), "recovering");
        assert_eq!(NodeStatus::Draining.to_string(), "draining");
        assert_eq!(NodeStatus::Maintenance.to_string(), "maintenance");
    }

    #[test]
    fn test_fault_tolerance_config_default() {
        let config = FaultToleranceConfig::default();
        assert_eq!(config.offline_threshold.as_secs(), 5 * 60);
        assert_eq!(config.drain_threshold.as_secs(), 4 * 60 * 60);
        assert_eq!(config.remove_threshold.as_secs(), 7 * 24 * 60 * 60);
        assert_eq!(config.recovery_quarantine.as_secs(), 5 * 60);
    }
}
