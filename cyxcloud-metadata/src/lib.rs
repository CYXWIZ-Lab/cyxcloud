//! CyxCloud Metadata Service
//!
//! Manages file and chunk metadata using PostgreSQL for persistence
//! and Redis for caching.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      MetadataService                             │
//! │                                                                  │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
//! │  │   Database   │  │    Cache     │  │  QuorumCoordinator   │  │
//! │  │  (PostgreSQL)│  │   (Redis)    │  │                      │  │
//! │  └──────────────┘  └──────────────┘  └──────────────────────┘  │
//! │         │                  │                    │               │
//! │  ┌──────────────────────────────────────────────────────────┐  │
//! │  │                   PlacementEngine                         │  │
//! │  │              (Topology-aware placement)                   │  │
//! │  └──────────────────────────────────────────────────────────┘  │
//! │         │                                                       │
//! │  ┌──────────────────────────────────────────────────────────┐  │
//! │  │                   HealthMonitor                           │  │
//! │  │              (Node health tracking)                       │  │
//! │  └──────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use cyxcloud_metadata::{MetadataService, MetadataConfig};
//!
//! let config = MetadataConfig::default();
//! let service = MetadataService::new(config).await?;
//!
//! // Register a file
//! let file = service.register_file(CreateFile { ... }).await?;
//!
//! // Get chunk locations
//! let nodes = service.get_chunk_locations(&chunk_id).await?;
//! ```

pub mod cache;
pub mod health;
pub mod models;
pub mod postgres;
pub mod quorum;
pub mod topology;

pub use cache::{Cache, CacheConfig, CacheError, OptionalCache};
pub use health::{HealthChecker, HealthConfig, HealthMonitor, HealthStatus, HealthSummary};
pub use models::*;
pub use postgres::{Database, DbConfig, DbError};
pub use quorum::{QuorumConfig, QuorumCoordinator, QuorumError, QuorumResult};
pub use topology::{PlacementConfig, PlacementEngine, PlacementNode, RebalanceSuggestion};

use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Metadata service error types
#[derive(Error, Debug)]
pub enum MetadataError {
    #[error("Database error: {0}")]
    Database(#[from] DbError),

    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Quorum error: {0}")]
    Quorum(#[from] QuorumError),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, MetadataError>;

/// Metadata service configuration
#[derive(Debug, Clone)]
pub struct MetadataConfig {
    /// PostgreSQL connection URL
    pub database_url: String,

    /// Redis connection URL (optional)
    pub redis_url: Option<String>,

    /// Database configuration
    pub db_config: DbConfig,

    /// Cache configuration
    pub cache_config: CacheConfig,

    /// Quorum configuration
    pub quorum_config: QuorumConfig,

    /// Placement configuration
    pub placement_config: PlacementConfig,

    /// Health check configuration
    pub health_config: HealthConfig,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://localhost/cyxcloud".to_string(),
            redis_url: Some("redis://localhost:6379".to_string()),
            db_config: DbConfig::default(),
            cache_config: CacheConfig::default(),
            quorum_config: QuorumConfig::default(),
            placement_config: PlacementConfig::default(),
            health_config: HealthConfig::default(),
        }
    }
}

impl MetadataConfig {
    /// Create config with just database URL
    pub fn with_database(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            database_url: url.clone(),
            db_config: DbConfig {
                url,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Disable Redis cache
    pub fn without_cache(mut self) -> Self {
        self.redis_url = None;
        self
    }
}

/// Main metadata service
pub struct MetadataService {
    /// Database connection
    db: Arc<Database>,

    /// Cache (optional)
    cache: OptionalCache,

    /// Quorum coordinator
    quorum: Arc<QuorumCoordinator>,

    /// Placement engine
    placement: Arc<PlacementEngine>,

    /// Health monitor
    health: Arc<HealthMonitor>,
}

impl MetadataService {
    /// Create a new metadata service
    pub async fn new(config: MetadataConfig) -> Result<Self> {
        // Connect to database
        let db = Database::new(DbConfig {
            url: config.database_url.clone(),
            ..config.db_config
        })
        .await?;

        // Connect to cache (if configured)
        let cache = if let Some(redis_url) = &config.redis_url {
            match Cache::new(CacheConfig {
                url: redis_url.clone(),
                ..config.cache_config
            })
            .await
            {
                Ok(cache) => {
                    info!("Redis cache connected");
                    OptionalCache::new(cache)
                }
                Err(e) => {
                    warn!(error = %e, "Failed to connect to Redis, running without cache");
                    OptionalCache::none()
                }
            }
        } else {
            OptionalCache::none()
        };

        let quorum = Arc::new(QuorumCoordinator::new(config.quorum_config));
        let placement = Arc::new(PlacementEngine::new(config.placement_config));
        let health = Arc::new(HealthMonitor::with_defaults());

        info!("MetadataService initialized");

        Ok(Self {
            db: Arc::new(db),
            cache,
            quorum,
            placement,
            health,
        })
    }

    /// Run database migrations
    pub async fn migrate(&self) -> Result<()> {
        self.db.migrate().await?;
        Ok(())
    }

    /// Get database reference
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// Get health monitor reference
    pub fn health_monitor(&self) -> Arc<HealthMonitor> {
        self.health.clone()
    }

    /// Get quorum coordinator reference
    pub fn quorum_coordinator(&self) -> Arc<QuorumCoordinator> {
        self.quorum.clone()
    }

    /// Get placement engine reference
    pub fn placement_engine(&self) -> Arc<PlacementEngine> {
        self.placement.clone()
    }

    // =========================================================================
    // NODE OPERATIONS
    // =========================================================================

    /// Register a new node
    pub async fn register_node(&self, node: CreateNode) -> Result<Node> {
        let result = self.db.create_node(node).await?;
        info!(node_id = %result.id, "Node registered");
        Ok(result)
    }

    /// Get or create node by peer ID
    pub async fn get_or_register_node(
        &self,
        peer_id: &str,
        grpc_address: &str,
    ) -> Result<Node> {
        if let Some(node) = self.db.get_node_by_peer_id(peer_id).await? {
            return Ok(node);
        }

        let node = self
            .db
            .create_node(CreateNode {
                peer_id: peer_id.to_string(),
                grpc_address: grpc_address.to_string(),
                storage_total: 0,
                bandwidth_mbps: 0,
                datacenter: None,
                region: None,
                version: None,
            })
            .await?;

        Ok(node)
    }

    /// Get online nodes
    pub async fn get_online_nodes(&self) -> Result<Vec<Node>> {
        // Try cache first
        if let Some(nodes) = self.cache.try_get::<Vec<Node>>("nodes:online").await {
            debug!("Got online nodes from cache");
            return Ok(nodes);
        }

        let nodes = self.db.get_online_nodes().await?;

        // Cache the result
        self.cache
            .try_set(
                "nodes:online",
                &nodes,
                self.cache.is_available().then(|| std::time::Duration::from_secs(60)).unwrap_or_default(),
            )
            .await;

        Ok(nodes)
    }

    /// Update node heartbeat
    pub async fn heartbeat(&self, node_id: Uuid) -> Result<()> {
        self.db.update_node_heartbeat(node_id).await?;
        Ok(())
    }

    /// Select nodes for placement
    pub async fn select_placement_nodes(
        &self,
        num_shards: usize,
        replicas_per_shard: usize,
    ) -> Result<Vec<Vec<String>>> {
        let nodes = self.get_online_nodes().await?;
        let placement_nodes: Vec<PlacementNode> = nodes.iter().map(PlacementNode::from_node).collect();

        let decisions = self
            .placement
            .select_nodes(&placement_nodes, num_shards, replicas_per_shard, None);

        Ok(decisions
            .into_iter()
            .map(|d| d.nodes.into_iter().map(|n| n.grpc_address).collect())
            .collect())
    }

    // =========================================================================
    // FILE OPERATIONS
    // =========================================================================

    /// Register a new file
    pub async fn register_file(&self, file: CreateFile) -> Result<File> {
        let result = self.db.create_file(file).await?;
        info!(file_id = %result.id, path = %result.path, "File registered");
        Ok(result)
    }

    /// Get file by ID
    pub async fn get_file(&self, file_id: Uuid) -> Result<Option<File>> {
        // Try cache first
        let cache_key = format!("file:{}", file_id);
        if let Some(file) = self.cache.try_get::<File>(&cache_key).await {
            return Ok(Some(file));
        }

        let file = self.db.get_file(file_id).await?;

        // Cache the result
        if let Some(ref f) = file {
            self.cache
                .try_set(&cache_key, f, std::time::Duration::from_secs(300))
                .await;
        }

        Ok(file)
    }

    /// Get file by path
    pub async fn get_file_by_path(&self, path: &str) -> Result<Option<File>> {
        let file = self.db.get_file_by_path(path).await?;
        Ok(file)
    }

    /// Mark file as complete
    pub async fn complete_file(&self, file_id: Uuid) -> Result<()> {
        self.db.update_file_status(file_id, "complete").await?;

        // Invalidate cache
        self.cache.try_delete(&format!("file:{}", file_id)).await;

        info!(file_id = %file_id, "File marked complete");
        Ok(())
    }

    /// Delete file (soft delete)
    pub async fn delete_file(&self, file_id: Uuid) -> Result<()> {
        self.db.delete_file(file_id).await?;

        // Invalidate cache
        self.cache.try_delete(&format!("file:{}", file_id)).await;

        info!(file_id = %file_id, "File deleted");
        Ok(())
    }

    // =========================================================================
    // CHUNK OPERATIONS
    // =========================================================================

    /// Register a chunk
    pub async fn register_chunk(&self, chunk: CreateChunk) -> Result<Chunk> {
        let result = self.db.create_chunk(chunk).await?;
        Ok(result)
    }

    /// Record chunk location
    pub async fn record_chunk_location(
        &self,
        chunk_id: &[u8],
        node_id: Uuid,
    ) -> Result<()> {
        self.db.add_chunk_location(chunk_id, node_id).await?;

        // Invalidate cache
        let cache_key = format!("chunk:{}", hex::encode(chunk_id));
        self.cache.try_delete(&cache_key).await;

        Ok(())
    }

    /// Get chunk locations (node addresses)
    pub async fn get_chunk_locations(&self, chunk_id: &[u8]) -> Result<Vec<String>> {
        // Try cache first
        let cache_key = format!("chunk:{}", hex::encode(chunk_id));
        if let Some(addrs) = self.cache.try_get::<Vec<String>>(&cache_key).await {
            return Ok(addrs);
        }

        let addrs = self.db.get_chunk_node_addresses(chunk_id).await?;

        // Cache the result
        self.cache
            .try_set(&cache_key, &addrs, std::time::Duration::from_secs(60))
            .await;

        Ok(addrs)
    }

    /// Get chunks for a file
    pub async fn get_file_chunks(&self, file_id: Uuid) -> Result<Vec<Chunk>> {
        let chunks = self.db.get_file_chunks(file_id).await?;
        Ok(chunks)
    }

    /// Get under-replicated chunks
    pub async fn get_under_replicated_chunks(&self, limit: i64) -> Result<Vec<ChunkReplicationStatus>> {
        let chunks = self.db.get_under_replicated_chunks(limit).await?;
        Ok(chunks)
    }

    // =========================================================================
    // USER OPERATIONS
    // =========================================================================

    /// Get or create user by wallet
    pub async fn get_or_create_user(&self, wallet_address: &str) -> Result<User> {
        if let Some(user) = self.db.get_user_by_wallet(wallet_address).await? {
            return Ok(user);
        }

        let user = self
            .db
            .create_user(Some(wallet_address), None, None)
            .await?;

        info!(user_id = %user.id, wallet = %wallet_address, "User created");
        Ok(user)
    }

    // =========================================================================
    // BUCKET OPERATIONS
    // =========================================================================

    /// Create a bucket
    pub async fn create_bucket(&self, name: &str, owner_id: Uuid) -> Result<Bucket> {
        let bucket = self.db.create_bucket(name, owner_id).await?;
        info!(bucket = %name, owner = %owner_id, "Bucket created");
        Ok(bucket)
    }

    /// Get bucket by name
    pub async fn get_bucket(&self, name: &str) -> Result<Option<Bucket>> {
        let bucket = self.db.get_bucket(name).await?;
        Ok(bucket)
    }

    /// Delete a bucket
    ///
    /// Returns error if bucket is not empty.
    pub async fn delete_bucket(&self, name: &str) -> Result<()> {
        // Check if bucket is empty
        if !self.bucket_is_empty(name).await? {
            return Err(MetadataError::Invalid(format!(
                "Bucket '{}' is not empty",
                name
            )));
        }

        self.db.delete_bucket(name).await?;
        info!(bucket = %name, "Bucket deleted");
        Ok(())
    }

    /// Check if a bucket is empty (has no files)
    pub async fn bucket_is_empty(&self, name: &str) -> Result<bool> {
        let is_empty = self.db.bucket_is_empty(name).await?;
        Ok(is_empty)
    }

    /// Count files in a bucket
    pub async fn count_files_in_bucket(&self, name: &str) -> Result<i64> {
        let count = self.db.count_files_in_bucket(name).await?;
        Ok(count)
    }

    // =========================================================================
    // REPAIR OPERATIONS
    // =========================================================================

    /// Create a repair job
    pub async fn create_repair_job(
        &self,
        chunk_id: &[u8],
        source_node_id: Option<Uuid>,
        target_node_id: Uuid,
        priority: i32,
    ) -> Result<RepairJob> {
        let job = self
            .db
            .create_repair_job(chunk_id, source_node_id, target_node_id, priority)
            .await?;
        Ok(job)
    }

    /// Get pending repair jobs
    pub async fn get_pending_repairs(&self, limit: i64) -> Result<Vec<RepairJob>> {
        let jobs = self.db.get_pending_repair_jobs(limit).await?;
        Ok(jobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_config_default() {
        let config = MetadataConfig::default();
        assert!(config.redis_url.is_some());
        assert!(!config.database_url.is_empty());
    }

    #[test]
    fn test_metadata_config_without_cache() {
        let config = MetadataConfig::default().without_cache();
        assert!(config.redis_url.is_none());
    }

    #[test]
    fn test_metadata_config_with_database() {
        let config = MetadataConfig::with_database("postgres://test:test@localhost/test");
        assert!(config.database_url.contains("localhost/test"));
    }
}
