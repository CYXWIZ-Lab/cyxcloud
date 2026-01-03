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
            offline_threshold: Duration::from_secs(5 * 60), // 5 minutes
            drain_threshold: Duration::from_secs(4 * 60 * 60), // 4 hours
            remove_threshold: Duration::from_secs(7 * 24 * 60 * 60), // 7 days
            recovery_quarantine: Duration::from_secs(5 * 60), // 5 minutes
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
        let result = sqlx::query_as::<_, NodeStorageSummary>("SELECT * FROM node_storage_summary")
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
        let node = self
            .get_node(node_id)
            .await?
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
        let node = self
            .get_node_by_peer_id(peer_id)
            .await?
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
        let result =
            sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = $1 AND deleted_at IS NULL")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(result)
    }

    /// Get a file by path
    pub async fn get_file_by_path(&self, path: &str) -> Result<Option<File>> {
        let result =
            sqlx::query_as::<_, File>("SELECT * FROM files WHERE path = $1 AND deleted_at IS NULL")
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
            INSERT INTO chunks (chunk_id, file_id, chunk_index, shard_index, is_parity, size_bytes, replication_factor)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(&chunk.chunk_id)
        .bind(chunk.file_id)
        .bind(chunk.chunk_index)
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
            "SELECT * FROM chunks WHERE file_id = $1 ORDER BY chunk_index, shard_index",
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
    pub async fn get_under_replicated_chunks(
        &self,
        limit: i64,
    ) -> Result<Vec<ChunkReplicationStatus>> {
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

        // Update chunk replica count and activate chunk
        let new_count = self.increment_chunk_replicas(chunk_id).await?;

        // Mark chunk as active once it has at least one replica stored
        if new_count == 1 {
            self.update_chunk_status(chunk_id, "active").await?;
        }

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
        let result = sqlx::query_as::<_, User>("SELECT * FROM users WHERE wallet_address = $1")
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
        let result =
            sqlx::query_as::<_, Bucket>("SELECT * FROM buckets WHERE owner_id = $1 ORDER BY name")
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
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM files WHERE bucket = $1 AND deleted_at IS NULL")
                .bind(bucket_name)
                .fetch_one(&self.pool)
                .await?;
        Ok(count.0 == 0)
    }

    /// Count files in a bucket
    pub async fn count_files_in_bucket(&self, bucket_name: &str) -> Result<i64> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM files WHERE bucket = $1 AND deleted_at IS NULL")
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

    // =========================================================================
    // UPTIME & PAYMENT OPERATIONS
    // =========================================================================

    /// Create or get existing epoch uptime record for a node
    #[instrument(skip(self))]
    pub async fn create_or_get_epoch_uptime(
        &self,
        node_id: Uuid,
        epoch: i64,
        epoch_start: chrono::DateTime<chrono::Utc>,
    ) -> Result<NodeEpochUptime> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            r#"
            INSERT INTO node_epoch_uptime (node_id, epoch, epoch_start)
            VALUES ($1, $2, $3)
            ON CONFLICT (node_id, epoch) DO UPDATE SET updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(node_id)
        .bind(epoch)
        .bind(epoch_start)
        .fetch_one(&self.pool)
        .await?;

        debug!(node_id = %node_id, epoch = epoch, "Epoch uptime record created/retrieved");
        Ok(result)
    }

    /// Add online time to a node's epoch uptime record
    #[instrument(skip(self))]
    pub async fn update_uptime_online(
        &self,
        node_id: Uuid,
        epoch: i64,
        seconds: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE node_epoch_uptime
            SET seconds_online = seconds_online + $1,
                last_status_change = NOW(),
                updated_at = NOW()
            WHERE node_id = $2 AND epoch = $3
            "#,
        )
        .bind(seconds)
        .bind(node_id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;

        debug!(node_id = %node_id, epoch = epoch, seconds = seconds, "Added online time");
        Ok(())
    }

    /// Add offline time to a node's epoch uptime record
    #[instrument(skip(self))]
    pub async fn update_uptime_offline(
        &self,
        node_id: Uuid,
        epoch: i64,
        seconds: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE node_epoch_uptime
            SET seconds_offline = seconds_offline + $1,
                last_status_change = NOW(),
                updated_at = NOW()
            WHERE node_id = $2 AND epoch = $3
            "#,
        )
        .bind(seconds)
        .bind(node_id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;

        debug!(node_id = %node_id, epoch = epoch, seconds = seconds, "Added offline time");
        Ok(())
    }

    /// Get all uptime records for an epoch (for payment calculation)
    pub async fn get_epoch_uptime(&self, epoch: i64) -> Result<Vec<NodeEpochUptime>> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            "SELECT * FROM node_epoch_uptime WHERE epoch = $1 ORDER BY seconds_online DESC",
        )
        .bind(epoch)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get uptime record for a specific node in an epoch
    pub async fn get_node_epoch_uptime(
        &self,
        node_id: Uuid,
        epoch: i64,
    ) -> Result<Option<NodeEpochUptime>> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            "SELECT * FROM node_epoch_uptime WHERE node_id = $1 AND epoch = $2",
        )
        .bind(node_id)
        .bind(epoch)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get unpaid uptime records for an epoch
    pub async fn get_unpaid_epoch_uptime(&self, epoch: i64) -> Result<Vec<NodeEpochUptime>> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            r#"
            SELECT * FROM node_epoch_uptime
            WHERE epoch = $1 AND payment_allocated = FALSE
            ORDER BY seconds_online DESC
            "#,
        )
        .bind(epoch)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Mark payment as allocated for a node's epoch uptime
    #[instrument(skip(self))]
    pub async fn mark_payment_allocated(
        &self,
        node_id: Uuid,
        epoch: i64,
        amount: i64,
        tx_signature: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE node_epoch_uptime
            SET payment_allocated = TRUE,
                payment_amount = $1,
                payment_tx_signature = $2,
                updated_at = NOW()
            WHERE node_id = $3 AND epoch = $4
            "#,
        )
        .bind(amount)
        .bind(tx_signature)
        .bind(node_id)
        .bind(epoch)
        .execute(&self.pool)
        .await?;

        debug!(node_id = %node_id, epoch = epoch, amount = amount, "Payment allocated");
        Ok(())
    }

    /// End epoch uptime tracking (set epoch_end timestamp)
    pub async fn end_epoch_uptime(&self, epoch: i64) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE node_epoch_uptime
            SET epoch_end = NOW(), updated_at = NOW()
            WHERE epoch = $1
            "#,
        )
        .bind(epoch)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // =========================================================================
    // SLASHING OPERATIONS
    // =========================================================================

    /// Record a slashing event
    #[instrument(skip(self))]
    pub async fn record_slashing_event(
        &self,
        node_id: Uuid,
        epoch: i64,
        reason: &str,
        slash_percent: i16,
        slash_amount: Option<i64>,
        tx_signature: Option<&str>,
        details: Option<serde_json::Value>,
    ) -> Result<SlashingEvent> {
        let result = sqlx::query_as::<_, SlashingEvent>(
            r#"
            INSERT INTO node_slashing_events (node_id, epoch, reason, slash_percent, slash_amount, tx_signature, details)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(node_id)
        .bind(epoch)
        .bind(reason)
        .bind(slash_percent)
        .bind(slash_amount)
        .bind(tx_signature)
        .bind(details)
        .fetch_one(&self.pool)
        .await?;

        debug!(node_id = %node_id, epoch = epoch, reason = reason, "Slashing event recorded");
        Ok(result)
    }

    /// Get slashing events for a node
    pub async fn get_node_slashing_events(&self, node_id: Uuid) -> Result<Vec<SlashingEvent>> {
        let result = sqlx::query_as::<_, SlashingEvent>(
            "SELECT * FROM node_slashing_events WHERE node_id = $1 ORDER BY created_at DESC",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get slashing events for an epoch
    pub async fn get_epoch_slashing_events(&self, epoch: i64) -> Result<Vec<SlashingEvent>> {
        let result = sqlx::query_as::<_, SlashingEvent>(
            "SELECT * FROM node_slashing_events WHERE epoch = $1 ORDER BY created_at",
        )
        .bind(epoch)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    // =========================================================================
    // PAYMENT EPOCH OPERATIONS
    // =========================================================================

    /// Create a new payment epoch
    #[instrument(skip(self))]
    pub async fn create_payment_epoch(&self, epoch: i64) -> Result<PaymentEpoch> {
        let result = sqlx::query_as::<_, PaymentEpoch>(
            r#"
            INSERT INTO payment_epochs (epoch, started_at)
            VALUES ($1, NOW())
            RETURNING *
            "#,
        )
        .bind(epoch)
        .fetch_one(&self.pool)
        .await?;

        debug!(epoch = epoch, "Payment epoch created");
        Ok(result)
    }

    /// Get payment epoch by number
    pub async fn get_payment_epoch(&self, epoch: i64) -> Result<Option<PaymentEpoch>> {
        let result =
            sqlx::query_as::<_, PaymentEpoch>("SELECT * FROM payment_epochs WHERE epoch = $1")
                .bind(epoch)
                .fetch_optional(&self.pool)
                .await?;
        Ok(result)
    }

    /// Get the current (latest non-finalized) payment epoch
    pub async fn get_current_payment_epoch(&self) -> Result<Option<PaymentEpoch>> {
        let result = sqlx::query_as::<_, PaymentEpoch>(
            "SELECT * FROM payment_epochs WHERE finalized = FALSE ORDER BY epoch DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Finalize a payment epoch
    #[instrument(skip(self))]
    pub async fn finalize_payment_epoch(
        &self,
        epoch: i64,
        total_pool_amount: i64,
        nodes_share: i64,
        platform_share: i64,
        community_share: i64,
        nodes_paid: i32,
        tx_signature: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE payment_epochs
            SET finalized = TRUE,
                ended_at = NOW(),
                total_pool_amount = $1,
                nodes_share = $2,
                platform_share = $3,
                community_share = $4,
                nodes_paid = $5,
                finalize_tx_signature = $6
            WHERE epoch = $7
            "#,
        )
        .bind(total_pool_amount)
        .bind(nodes_share)
        .bind(platform_share)
        .bind(community_share)
        .bind(nodes_paid)
        .bind(tx_signature)
        .bind(epoch)
        .execute(&self.pool)
        .await?;

        debug!(
            epoch = epoch,
            nodes_paid = nodes_paid,
            "Payment epoch finalized"
        );
        Ok(())
    }

    /// Mark platform share as claimed
    pub async fn mark_platform_claimed(&self, epoch: i64) -> Result<()> {
        sqlx::query("UPDATE payment_epochs SET platform_claimed = TRUE WHERE epoch = $1")
            .bind(epoch)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark community share as claimed
    pub async fn mark_community_claimed(&self, epoch: i64) -> Result<()> {
        sqlx::query("UPDATE payment_epochs SET community_claimed = TRUE WHERE epoch = $1")
            .bind(epoch)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get nodes with extended downtime (>4 hours offline in an epoch)
    /// These nodes should be slashed for extended downtime
    pub async fn get_nodes_with_extended_downtime(
        &self,
        epoch: i64,
        threshold_seconds: i64,
    ) -> Result<Vec<NodeEpochUptime>> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            r#"
            SELECT * FROM node_epoch_uptime
            WHERE epoch = $1 AND seconds_offline > $2
            "#,
        )
        .bind(epoch)
        .bind(threshold_seconds)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get payment history for a node
    pub async fn get_node_payment_history(
        &self,
        node_id: Uuid,
        limit: i64,
    ) -> Result<Vec<NodeEpochUptime>> {
        let result = sqlx::query_as::<_, NodeEpochUptime>(
            r#"
            SELECT * FROM node_epoch_uptime
            WHERE node_id = $1 AND payment_allocated = TRUE
            ORDER BY epoch DESC
            LIMIT $2
            "#,
        )
        .bind(node_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Initialize epoch uptime records for all active nodes
    /// Call this at the start of each epoch to ensure all nodes have records
    #[instrument(skip(self))]
    pub async fn initialize_epoch_for_all_nodes(
        &self,
        epoch: i64,
        epoch_start: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64> {
        let result = sqlx::query(
            r#"
            INSERT INTO node_epoch_uptime (node_id, epoch, epoch_start)
            SELECT id, $1, $2 FROM nodes
            WHERE status IN ('online', 'recovering', 'offline', 'draining')
            ON CONFLICT (node_id, epoch) DO NOTHING
            "#,
        )
        .bind(epoch)
        .bind(epoch_start)
        .execute(&self.pool)
        .await?;

        let count = result.rows_affected();
        debug!(
            epoch = epoch,
            nodes_initialized = count,
            "Initialized epoch for all nodes"
        );
        Ok(count)
    }

    // =========================================================================
    // DATASTREAM OPERATIONS
    // =========================================================================

    /// Create a new dataset
    #[instrument(skip(self, dataset))]
    pub async fn create_dataset(&self, dataset: CreateDataset) -> Result<Dataset> {
        let result = sqlx::query_as::<_, Dataset>(
            r#"
            INSERT INTO datasets (name, owner_id, description, content_hash, total_size_bytes,
                                 file_count, schema, trust_level, signature, parent_version_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
        )
        .bind(&dataset.name)
        .bind(dataset.owner_id)
        .bind(&dataset.description)
        .bind(&dataset.content_hash)
        .bind(dataset.total_size_bytes)
        .bind(dataset.file_count)
        .bind(&dataset.schema)
        .bind(dataset.trust_level as i32)
        .bind(&dataset.signature)
        .bind(dataset.parent_version_id)
        .fetch_one(&self.pool)
        .await?;

        debug!(dataset_id = %result.id, name = %dataset.name, "Dataset created");
        Ok(result)
    }

    /// Get a dataset by ID
    pub async fn get_dataset(&self, id: Uuid) -> Result<Option<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>("SELECT * FROM datasets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(result)
    }

    /// Get datasets by owner
    pub async fn get_datasets_by_owner(&self, owner_id: Uuid) -> Result<Vec<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            "SELECT * FROM datasets WHERE owner_id = $1 ORDER BY created_at DESC",
        )
        .bind(owner_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get dataset by name and owner (latest version)
    pub async fn get_dataset_by_name(
        &self,
        owner_id: Uuid,
        name: &str,
    ) -> Result<Option<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            "SELECT * FROM datasets WHERE owner_id = $1 AND name = $2 ORDER BY version DESC LIMIT 1",
        )
        .bind(owner_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get all versions of a dataset
    pub async fn get_dataset_versions(&self, owner_id: Uuid, name: &str) -> Result<Vec<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            "SELECT * FROM datasets WHERE owner_id = $1 AND name = $2 ORDER BY version DESC",
        )
        .bind(owner_id)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get datasets by trust level
    pub async fn get_datasets_by_trust_level(&self, trust_level: i32) -> Result<Vec<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            "SELECT * FROM datasets WHERE trust_level <= $1 ORDER BY created_at DESC",
        )
        .bind(trust_level)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update dataset trust level and verification
    pub async fn update_dataset_trust(
        &self,
        dataset_id: Uuid,
        trust_level: i32,
        signature: Option<&[u8]>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE datasets
            SET trust_level = $1, signature = $2, verified_at = NOW(), updated_at = NOW()
            WHERE id = $3
            "#,
        )
        .bind(trust_level)
        .bind(signature)
        .bind(dataset_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete a dataset
    pub async fn delete_dataset(&self, dataset_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM datasets WHERE id = $1")
            .bind(dataset_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Add a file to a dataset
    #[instrument(skip(self, file))]
    pub async fn create_dataset_file(&self, file: CreateDatasetFile) -> Result<DatasetFile> {
        let result = sqlx::query_as::<_, DatasetFile>(
            r#"
            INSERT INTO dataset_files (dataset_id, file_id, path_in_dataset, content_hash, size_bytes, file_index)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(file.dataset_id)
        .bind(file.file_id)
        .bind(&file.path_in_dataset)
        .bind(&file.content_hash)
        .bind(file.size_bytes)
        .bind(file.file_index)
        .fetch_one(&self.pool)
        .await?;

        debug!(dataset_id = %file.dataset_id, file_id = %file.file_id, "Dataset file added");
        Ok(result)
    }

    /// Get all files in a dataset
    pub async fn get_dataset_files(&self, dataset_id: Uuid) -> Result<Vec<DatasetFile>> {
        let result = sqlx::query_as::<_, DatasetFile>(
            "SELECT * FROM dataset_files WHERE dataset_id = $1 ORDER BY file_index",
        )
        .bind(dataset_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get a specific file in a dataset by path
    pub async fn get_dataset_file_by_path(
        &self,
        dataset_id: Uuid,
        path: &str,
    ) -> Result<Option<DatasetFile>> {
        let result = sqlx::query_as::<_, DatasetFile>(
            "SELECT * FROM dataset_files WHERE dataset_id = $1 AND path_in_dataset = $2",
        )
        .bind(dataset_id)
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get a file by index (for streaming batches)
    pub async fn get_dataset_file_by_index(
        &self,
        dataset_id: Uuid,
        index: i32,
    ) -> Result<Option<DatasetFile>> {
        let result = sqlx::query_as::<_, DatasetFile>(
            "SELECT * FROM dataset_files WHERE dataset_id = $1 AND file_index = $2",
        )
        .bind(dataset_id)
        .bind(index)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get public dataset by name and version
    pub async fn get_public_dataset(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<PublicDataset>> {
        let result = sqlx::query_as::<_, PublicDataset>(
            "SELECT * FROM public_datasets WHERE name = $1 AND version = $2",
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get all public datasets
    pub async fn get_all_public_datasets(&self) -> Result<Vec<PublicDataset>> {
        let result =
            sqlx::query_as::<_, PublicDataset>("SELECT * FROM public_datasets ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        Ok(result)
    }

    /// Find public dataset by hash
    pub async fn find_public_dataset_by_hash(
        &self,
        hash: &[u8],
    ) -> Result<Option<PublicDataset>> {
        let result = sqlx::query_as::<_, PublicDataset>(
            "SELECT * FROM public_datasets WHERE official_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Update public dataset cache reference
    pub async fn update_public_dataset_cache(
        &self,
        public_dataset_id: Uuid,
        cached_dataset_id: Uuid,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE public_datasets SET cached_dataset_id = $1, cached_at = NOW() WHERE id = $2",
        )
        .bind(cached_dataset_id)
        .bind(public_dataset_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Create a data access token
    #[instrument(skip(self, token))]
    pub async fn create_data_access_token(
        &self,
        token: CreateDataAccessToken,
    ) -> Result<DataAccessToken> {
        let result = sqlx::query_as::<_, DataAccessToken>(
            r#"
            INSERT INTO data_access_tokens (id, dataset_id, node_id, user_id, token_hash, scopes, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(token.id)
        .bind(token.dataset_id)
        .bind(token.node_id)
        .bind(token.user_id)
        .bind(&token.token_hash)
        .bind(&token.scopes)
        .bind(token.expires_at)
        .fetch_one(&self.pool)
        .await?;

        debug!(token_id = %result.id, dataset_id = %token.dataset_id, "Data access token created");
        Ok(result)
    }

    /// Get data access token by ID
    pub async fn get_data_access_token(&self, id: Uuid) -> Result<Option<DataAccessToken>> {
        let result = sqlx::query_as::<_, DataAccessToken>(
            "SELECT * FROM data_access_tokens WHERE id = $1 AND revoked_at IS NULL AND expires_at > NOW()",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Validate a data access token by hash
    pub async fn validate_data_access_token(
        &self,
        token_hash: &[u8],
    ) -> Result<Option<DataAccessToken>> {
        let result = sqlx::query_as::<_, DataAccessToken>(
            "SELECT * FROM data_access_tokens WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > NOW()",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Revoke a data access token
    pub async fn revoke_data_access_token(&self, token_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE data_access_tokens SET revoked_at = NOW() WHERE id = $1")
            .bind(token_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke all tokens for a dataset
    pub async fn revoke_dataset_tokens(&self, dataset_id: Uuid) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE data_access_tokens SET revoked_at = NOW() WHERE dataset_id = $1 AND revoked_at IS NULL",
        )
        .bind(dataset_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Clean up expired tokens
    pub async fn cleanup_expired_tokens(&self) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM data_access_tokens WHERE expires_at < NOW() - INTERVAL '7 days'")
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    /// Share a dataset with another user
    #[instrument(skip(self, share))]
    pub async fn create_dataset_share(&self, share: CreateDatasetShare) -> Result<DatasetShare> {
        let result = sqlx::query_as::<_, DatasetShare>(
            r#"
            INSERT INTO dataset_shares (dataset_id, shared_with_user_id, shared_by_user_id, permissions, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (dataset_id, shared_with_user_id) DO UPDATE SET
                permissions = EXCLUDED.permissions,
                expires_at = EXCLUDED.expires_at
            RETURNING *
            "#,
        )
        .bind(share.dataset_id)
        .bind(share.shared_with_user_id)
        .bind(share.shared_by_user_id)
        .bind(&share.permissions)
        .bind(share.expires_at)
        .fetch_one(&self.pool)
        .await?;

        debug!(
            dataset_id = %share.dataset_id,
            shared_with = %share.shared_with_user_id,
            "Dataset shared"
        );
        Ok(result)
    }

    /// Get shares for a dataset
    pub async fn get_dataset_shares(&self, dataset_id: Uuid) -> Result<Vec<DatasetShare>> {
        let result = sqlx::query_as::<_, DatasetShare>(
            "SELECT * FROM dataset_shares WHERE dataset_id = $1 AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .bind(dataset_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get datasets shared with a user
    pub async fn get_datasets_shared_with_user(&self, user_id: Uuid) -> Result<Vec<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            r#"
            SELECT d.* FROM datasets d
            JOIN dataset_shares ds ON ds.dataset_id = d.id
            WHERE ds.shared_with_user_id = $1
            AND (ds.expires_at IS NULL OR ds.expires_at > NOW())
            ORDER BY d.created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
    }

    /// Check if user has access to a dataset
    pub async fn check_dataset_access(
        &self,
        dataset_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<DatasetShare>> {
        let result = sqlx::query_as::<_, DatasetShare>(
            r#"
            SELECT * FROM dataset_shares
            WHERE dataset_id = $1 AND shared_with_user_id = $2
            AND (expires_at IS NULL OR expires_at > NOW())
            "#,
        )
        .bind(dataset_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Remove a dataset share
    pub async fn remove_dataset_share(
        &self,
        dataset_id: Uuid,
        shared_with_user_id: Uuid,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM dataset_shares WHERE dataset_id = $1 AND shared_with_user_id = $2",
        )
        .bind(dataset_id)
        .bind(shared_with_user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get dataset summary with trust info
    pub async fn get_dataset_summary(&self, dataset_id: Uuid) -> Result<Option<DatasetSummary>> {
        let result = sqlx::query_as::<_, DatasetSummary>(
            "SELECT * FROM dataset_summary WHERE id = $1",
        )
        .bind(dataset_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Get all accessible datasets for a user (owned + shared)
    pub async fn get_user_accessible_datasets(&self, user_id: Uuid) -> Result<Vec<Dataset>> {
        let result = sqlx::query_as::<_, Dataset>(
            r#"
            SELECT * FROM datasets WHERE owner_id = $1
            UNION
            SELECT d.* FROM datasets d
            JOIN dataset_shares ds ON ds.dataset_id = d.id
            WHERE ds.shared_with_user_id = $1
            AND (ds.expires_at IS NULL OR ds.expires_at > NOW())
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(result)
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
