//! Application State
//!
//! Shared state for all gateway components.
//! Integrates with cyxcloud-metadata for persistent storage.
//! Uses NodeClient for chunk operations with storage nodes.
//! Optionally integrates with Solana blockchain for payments.

#![allow(unused_imports)]
#![allow(unused_variables)]

use bytes::Bytes;
use cyxcloud_core::{
    crypto::ContentHash, reassemble_chunks, split_into_chunks, ErasureEncoder, ShardData,
    DATA_SHARDS, DEFAULT_CHUNK_SIZE, PARITY_SHARDS, TOTAL_SHARDS,
};
use cyxcloud_metadata::{
    CreateChunk, MetadataConfig, MetadataError, MetadataService, PlacementConfig, PlacementEngine,
    PlacementNode,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::auth::{AuthConfig, AuthService};
#[cfg(feature = "blockchain")]
use crate::blockchain::{BlockchainConfig, CyxCloudBlockchainClient};
use crate::node_client::{ChunkMeta, NodeClient, NodeClientConfig};
use crate::s3_api::{ObjectInfo, ObjectMetadata, S3Error, S3Result};
use crate::websocket::EventHub;

/// Maximum number of in-memory buckets (development mode)
const MAX_MEMORY_BUCKETS: usize = 1000;

/// Maximum total bytes stored in memory (256 MB)
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// PostgreSQL connection URL
    pub database_url: Option<String>,

    /// Redis connection URL
    pub redis_url: Option<String>,

    /// Use in-memory storage (for development/testing)
    pub use_memory_storage: bool,

    /// Enable blockchain integration
    #[cfg(feature = "blockchain")]
    pub enable_blockchain: bool,

    /// Solana RPC URL
    #[cfg(feature = "blockchain")]
    pub solana_rpc_url: Option<String>,

    /// Gateway authority keypair path
    #[cfg(feature = "blockchain")]
    pub keypair_path: Option<String>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            database_url: None,
            redis_url: None,
            use_memory_storage: true, // Default to memory for easy development
            #[cfg(feature = "blockchain")]
            enable_blockchain: false,
            #[cfg(feature = "blockchain")]
            solana_rpc_url: None,
            #[cfg(feature = "blockchain")]
            keypair_path: None,
        }
    }
}

impl GatewayConfig {
    /// Create config for production with database
    pub fn with_database(database_url: impl Into<String>) -> Self {
        Self {
            database_url: Some(database_url.into()),
            redis_url: None,
            use_memory_storage: false,
            #[cfg(feature = "blockchain")]
            enable_blockchain: false,
            #[cfg(feature = "blockchain")]
            solana_rpc_url: None,
            #[cfg(feature = "blockchain")]
            keypair_path: None,
        }
    }

    /// Create config from environment variables
    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL").ok();
        let redis_url = std::env::var("REDIS_URL").ok();
        let use_memory = std::env::var("USE_MEMORY_STORAGE")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(database_url.is_none());

        #[cfg(feature = "blockchain")]
        let enable_blockchain = std::env::var("ENABLE_BLOCKCHAIN")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);
        #[cfg(feature = "blockchain")]
        let solana_rpc_url = std::env::var("SOLANA_RPC_URL").ok();
        #[cfg(feature = "blockchain")]
        let keypair_path = std::env::var("GATEWAY_KEYPAIR_PATH").ok();

        Self {
            database_url,
            redis_url,
            use_memory_storage: use_memory,
            #[cfg(feature = "blockchain")]
            enable_blockchain,
            #[cfg(feature = "blockchain")]
            solana_rpc_url,
            #[cfg(feature = "blockchain")]
            keypair_path,
        }
    }
}

/// Application state shared across all handlers
pub struct AppState {
    /// Event hub for WebSocket broadcasts
    pub event_hub: Arc<EventHub>,

    /// Metadata service (when using database)
    metadata: Option<Arc<MetadataService>>,

    /// Node client for chunk operations
    node_client: Arc<NodeClient>,

    /// Authentication service
    auth: Arc<AuthService>,

    /// Blockchain client (optional, for Solana integration)
    #[cfg(feature = "blockchain")]
    blockchain: Option<Arc<CyxCloudBlockchainClient>>,

    /// In-memory bucket storage (for development)
    memory_buckets: RwLock<HashMap<String, BucketState>>,

    /// Total bytes stored in memory across all buckets
    memory_bytes_used: std::sync::atomic::AtomicUsize,

    /// User ID (would come from authentication)
    user_id: Uuid,

    /// Whether using memory storage
    use_memory: bool,
}

/// Bucket state for in-memory storage
struct BucketState {
    objects: HashMap<String, StoredObject>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Stored object for in-memory storage
struct StoredObject {
    data: Bytes,
    content_type: String,
    etag: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    /// Create a new application state with in-memory storage
    pub fn new() -> Self {
        Self {
            event_hub: Arc::new(EventHub::new(1024)),
            metadata: None,
            node_client: Arc::new(NodeClient::new(NodeClientConfig::default())),
            auth: Arc::new(AuthService::from_env()),
            #[cfg(feature = "blockchain")]
            blockchain: None,
            memory_buckets: RwLock::new(HashMap::new()),
            memory_bytes_used: std::sync::atomic::AtomicUsize::new(0),
            user_id: Uuid::new_v4(),
            use_memory: true,
        }
    }

    /// Create application state with configuration
    pub async fn with_config(config: GatewayConfig) -> Result<Self, MetadataError> {
        let metadata = if let Some(ref db_url) = config.database_url {
            if !config.use_memory_storage {
                let meta_config = MetadataConfig {
                    database_url: db_url.clone(),
                    redis_url: config.redis_url.clone(),
                    ..Default::default()
                };

                match MetadataService::new(meta_config).await {
                    Ok(service) => {
                        info!("Connected to metadata database");
                        // Run migrations
                        if let Err(e) = service.migrate().await {
                            warn!(error = %e, "Failed to run migrations");
                        }
                        Some(Arc::new(service))
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to connect to database, falling back to memory");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let use_memory = metadata.is_none();
        if use_memory {
            info!("Using in-memory storage");
        }

        // Initialize blockchain client if enabled (requires 'blockchain' feature)
        #[cfg(feature = "blockchain")]
        let blockchain = if config.enable_blockchain {
            match Self::init_blockchain_client(&config) {
                Ok(client) => {
                    info!("Blockchain client initialized");
                    Some(Arc::new(client))
                }
                Err(e) => {
                    warn!(error = %e, "Failed to initialize blockchain client");
                    None
                }
            }
        } else {
            debug!("Blockchain integration disabled");
            None
        };

        Ok(Self {
            event_hub: Arc::new(EventHub::new(1024)),
            metadata,
            node_client: Arc::new(NodeClient::new(NodeClientConfig::default())),
            auth: Arc::new(AuthService::from_env()),
            #[cfg(feature = "blockchain")]
            blockchain,
            memory_buckets: RwLock::new(HashMap::new()),
            memory_bytes_used: std::sync::atomic::AtomicUsize::new(0),
            user_id: Uuid::new_v4(),
            use_memory,
        })
    }

    /// Initialize blockchain client from config
    #[cfg(feature = "blockchain")]
    fn init_blockchain_client(config: &GatewayConfig) -> anyhow::Result<CyxCloudBlockchainClient> {
        use solana_sdk::signature::Keypair;

        // Build blockchain config
        let bc_config = BlockchainConfig {
            rpc_url: config
                .solana_rpc_url
                .clone()
                .unwrap_or_else(|| "https://api.devnet.solana.com".to_string()),
            keypair_path: config.keypair_path.clone(),
            ..Default::default()
        };

        // Load or generate keypair
        let authority = if let Some(ref path) = config.keypair_path {
            let keypair_json = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("Failed to read keypair file: {}", e))?;
            let keypair_bytes: Vec<u8> = serde_json::from_str(&keypair_json)
                .map_err(|e| anyhow::anyhow!("Failed to parse keypair JSON: {}", e))?;
            #[allow(deprecated)]
            Keypair::from_bytes(&keypair_bytes)
                .map_err(|e| anyhow::anyhow!("Failed to parse keypair: {}", e))?
        } else {
            warn!("No keypair path configured, using ephemeral keypair (not recommended for production)");
            Keypair::new()
        };

        CyxCloudBlockchainClient::new(bc_config, authority)
    }

    /// Get metadata service reference
    pub fn metadata_service(&self) -> Option<&MetadataService> {
        self.metadata.as_ref().map(|m| m.as_ref())
    }

    /// Get cloned Arc to metadata service (for async operations)
    pub fn metadata_service_arc(&self) -> Option<Arc<MetadataService>> {
        self.metadata.clone()
    }

    /// Get node client reference
    pub fn node_client(&self) -> &NodeClient {
        &self.node_client
    }

    /// Get cloned Arc to node client (for async operations)
    pub fn node_client_arc(&self) -> Arc<NodeClient> {
        self.node_client.clone()
    }

    /// Get authentication service reference
    pub fn auth_service(&self) -> &AuthService {
        &self.auth
    }

    /// Get cloned Arc to auth service
    pub fn auth_service_arc(&self) -> Arc<AuthService> {
        self.auth.clone()
    }

    /// Get blockchain client reference
    #[cfg(feature = "blockchain")]
    pub fn blockchain_client(&self) -> Option<&CyxCloudBlockchainClient> {
        self.blockchain.as_ref().map(|b| b.as_ref())
    }

    /// Get cloned Arc to blockchain client
    #[cfg(feature = "blockchain")]
    pub fn blockchain_client_arc(&self) -> Option<Arc<CyxCloudBlockchainClient>> {
        self.blockchain.clone()
    }

    /// Check if blockchain integration is enabled
    #[cfg(feature = "blockchain")]
    pub fn has_blockchain(&self) -> bool {
        self.blockchain.is_some()
    }

    /// Check if blockchain integration is enabled (always false when feature not enabled)
    #[cfg(not(feature = "blockchain"))]
    pub fn has_blockchain(&self) -> bool {
        false
    }

    // =========================================================================
    // BLOCKCHAIN USAGE TRACKING
    // =========================================================================

    /// Update on-chain storage usage for a user
    ///
    /// This should be called after successful upload or deletion operations
    /// to sync usage data with the Solana blockchain.
    #[cfg(feature = "blockchain")]
    pub async fn update_blockchain_usage(
        &self,
        wallet_address: &str,
        storage_delta: i64,
        bandwidth_used: u64,
    ) -> Result<(), String> {
        use solana_sdk::pubkey::Pubkey;
        use std::str::FromStr;

        let blockchain = match &self.blockchain {
            Some(bc) => bc,
            None => {
                debug!("Blockchain not enabled, skipping usage update");
                return Ok(());
            }
        };

        // Parse wallet address as Solana Pubkey
        let user_pubkey = Pubkey::from_str(wallet_address)
            .map_err(|e| format!("Invalid wallet address '{}': {}", wallet_address, e))?;

        // Get current subscription to calculate new totals
        let subscription = match blockchain.get_subscription(&user_pubkey).await {
            Ok(Some(sub)) => sub,
            Ok(None) => {
                debug!(
                    wallet = wallet_address,
                    "No subscription found, skipping usage update"
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    wallet = wallet_address,
                    error = %e,
                    "Failed to get subscription for usage update"
                );
                return Err(format!("Failed to get subscription: {}", e));
            }
        };

        // Calculate new storage used (handle negative delta for deletions)
        let new_storage_used = if storage_delta >= 0 {
            subscription
                .storage_used_bytes
                .saturating_add(storage_delta as u64)
        } else {
            subscription
                .storage_used_bytes
                .saturating_sub((-storage_delta) as u64)
        };

        // Calculate new bandwidth used (always additive)
        let new_bandwidth_used = subscription
            .bandwidth_used_bytes
            .saturating_add(bandwidth_used);

        // Update on-chain
        match blockchain
            .update_usage(&user_pubkey, new_storage_used, new_bandwidth_used)
            .await
        {
            Ok(sig) => {
                info!(
                    wallet = wallet_address,
                    storage_used = new_storage_used,
                    bandwidth_used = new_bandwidth_used,
                    signature = %sig,
                    "Blockchain usage updated"
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    wallet = wallet_address,
                    error = %e,
                    "Failed to update blockchain usage"
                );
                Err(format!("Blockchain update failed: {}", e))
            }
        }
    }

    /// No-op when blockchain feature is disabled
    #[cfg(not(feature = "blockchain"))]
    pub async fn update_blockchain_usage(
        &self,
        _wallet_address: &str,
        _storage_delta: i64,
        _bandwidth_used: u64,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Check if user has sufficient quota for an upload
    #[cfg(feature = "blockchain")]
    pub async fn check_blockchain_quota(
        &self,
        wallet_address: &str,
        bytes_needed: u64,
    ) -> Result<bool, String> {
        use solana_sdk::pubkey::Pubkey;
        use std::str::FromStr;

        let blockchain = match &self.blockchain {
            Some(bc) => bc,
            None => {
                debug!("Blockchain not enabled, quota check passed by default");
                return Ok(true);
            }
        };

        let user_pubkey = Pubkey::from_str(wallet_address)
            .map_err(|e| format!("Invalid wallet address: {}", e))?;

        match blockchain.has_quota(&user_pubkey, bytes_needed).await {
            Ok(has_quota) => {
                if !has_quota {
                    warn!(
                        wallet = wallet_address,
                        bytes_needed = bytes_needed,
                        "User does not have sufficient storage quota"
                    );
                }
                Ok(has_quota)
            }
            Err(e) => {
                warn!(
                    wallet = wallet_address,
                    error = %e,
                    "Failed to check quota, allowing by default"
                );
                // Allow by default if we can't verify quota
                Ok(true)
            }
        }
    }

    /// No-op when blockchain feature is disabled
    #[cfg(not(feature = "blockchain"))]
    pub async fn check_blockchain_quota(
        &self,
        _wallet_address: &str,
        _bytes_needed: u64,
    ) -> Result<bool, String> {
        Ok(true)
    }

    // =========================================================================
    // BUCKET OPERATIONS
    // =========================================================================

    /// Check if bucket exists
    pub async fn bucket_exists(&self, name: &str) -> S3Result<bool> {
        if self.use_memory {
            let buckets = self.memory_buckets.read().await;
            return Ok(buckets.contains_key(name));
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            match meta.get_bucket(name).await {
                Ok(bucket) => Ok(bucket.is_some()),
                Err(e) => {
                    warn!(error = %e, bucket = name, "Failed to check bucket existence");
                    Err(S3Error::Internal(e.to_string()))
                }
            }
        } else {
            Ok(false)
        }
    }

    /// Create a bucket
    pub async fn create_bucket(&self, name: &str) -> S3Result<()> {
        if self.use_memory {
            let mut buckets = self.memory_buckets.write().await;

            if buckets.contains_key(name) {
                return Err(S3Error::BucketAlreadyExists(name.to_string()));
            }

            if buckets.len() >= MAX_MEMORY_BUCKETS {
                return Err(S3Error::Internal(format!(
                    "Maximum number of in-memory buckets ({}) reached",
                    MAX_MEMORY_BUCKETS
                )));
            }

            buckets.insert(
                name.to_string(),
                BucketState {
                    objects: HashMap::new(),
                    created_at: chrono::Utc::now(),
                },
            );

            info!(bucket = name, "Bucket created (memory)");
            return Ok(());
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            // Check if exists
            if let Ok(Some(_)) = meta.get_bucket(name).await {
                return Err(S3Error::BucketAlreadyExists(name.to_string()));
            }

            // Get or create user
            let user = meta
                .get_or_create_user(&self.user_id.to_string())
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            // Create bucket
            meta.create_bucket(name, user.id)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            info!(bucket = name, "Bucket created (database)");
            Ok(())
        } else {
            Err(S3Error::Internal(
                "No storage backend available".to_string(),
            ))
        }
    }

    /// Delete a bucket
    pub async fn delete_bucket(&self, name: &str) -> S3Result<()> {
        if self.use_memory {
            let mut buckets = self.memory_buckets.write().await;
            buckets.remove(name);
            info!(bucket = name, "Bucket deleted (memory)");
            return Ok(());
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            // Check if bucket exists
            let bucket = meta
                .get_bucket(name)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            if bucket.is_none() {
                return Err(S3Error::NoSuchBucket(name.to_string()));
            }

            // Check if bucket is empty
            let is_empty = meta
                .bucket_is_empty(name)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            if !is_empty {
                return Err(S3Error::BucketNotEmpty(name.to_string()));
            }

            // Delete the bucket
            meta.delete_bucket(name)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            info!(bucket = name, "Bucket deleted (database)");
            return Ok(());
        }

        Err(S3Error::Internal(
            "No storage backend available".to_string(),
        ))
    }

    /// Check if bucket is empty
    pub async fn bucket_is_empty(&self, name: &str) -> S3Result<bool> {
        if self.use_memory {
            let buckets = self.memory_buckets.read().await;
            let bucket = buckets
                .get(name)
                .ok_or_else(|| S3Error::NoSuchBucket(name.to_string()))?;
            return Ok(bucket.objects.is_empty());
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            let is_empty = meta
                .bucket_is_empty(name)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;
            return Ok(is_empty);
        }

        // No backend, assume empty
        Ok(true)
    }

    // =========================================================================
    // OBJECT OPERATIONS
    // =========================================================================

    /// Put an object
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        data: Bytes,
        content_type: &str,
    ) -> S3Result<String> {
        if self.use_memory {
            let new_size = data.len();

            // Check memory limit
            let current_bytes = self
                .memory_bytes_used
                .load(std::sync::atomic::Ordering::Relaxed);
            if current_bytes + new_size > MAX_MEMORY_BYTES {
                return Err(S3Error::Internal(format!(
                    "In-memory storage limit ({} MB) exceeded",
                    MAX_MEMORY_BYTES / (1024 * 1024)
                )));
            }

            let mut buckets = self.memory_buckets.write().await;
            let bucket_state = buckets
                .get_mut(bucket)
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            // Calculate ETag (MD5 hash)
            let etag = format!("{:x}", md5::compute(&data));

            // Track size delta (subtract old object size if overwriting)
            let old_size = bucket_state
                .objects
                .get(key)
                .map(|o| o.data.len())
                .unwrap_or(0);

            bucket_state.objects.insert(
                key.to_string(),
                StoredObject {
                    data,
                    content_type: content_type.to_string(),
                    etag: etag.clone(),
                    created_at: chrono::Utc::now(),
                },
            );

            // Update tracked memory usage
            if new_size >= old_size {
                self.memory_bytes_used
                    .fetch_add(new_size - old_size, std::sync::atomic::Ordering::Relaxed);
            } else {
                self.memory_bytes_used
                    .fetch_sub(old_size - new_size, std::sync::atomic::Ordering::Relaxed);
            }

            // Publish event
            drop(buckets);
            self.publish_file_created(bucket, key, 0).await;

            return Ok(etag);
        }

        // Use metadata service + node storage with erasure coding
        if let Some(ref meta) = self.metadata {
            // Get bucket info
            let bucket_info = meta
                .get_bucket(bucket)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            // Get available nodes
            let nodes = meta
                .get_online_nodes()
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            // Need at least TOTAL_SHARDS (14) nodes for optimal distribution
            // but can work with fewer using replication
            if nodes.is_empty() {
                return Err(S3Error::Internal("No storage nodes available".to_string()));
            }

            // Create placement engine for smart node selection
            let placement_config = PlacementConfig::default();
            let placement_engine = PlacementEngine::new(placement_config);

            // Convert nodes to PlacementNodes for the engine
            let placement_nodes: Vec<PlacementNode> =
                nodes.iter().map(PlacementNode::from_node).collect();

            // Create file record
            let file_id = Uuid::new_v4();
            let content_hash = cyxcloud_core::ContentHash::compute(&data);

            // Create erasure encoder (10 data + 4 parity = 14 shards)
            let erasure_encoder = ErasureEncoder::new().map_err(|e| {
                S3Error::Internal(format!("Failed to create erasure encoder: {}", e))
            })?;

            // Split data into chunks
            let chunks = split_into_chunks(&data, DEFAULT_CHUNK_SIZE, Some(file_id))
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            let chunk_count = chunks.len();
            let total_shards = chunk_count * TOTAL_SHARDS;

            info!(
                bucket = bucket,
                key = key,
                size = data.len(),
                chunks = chunk_count,
                total_shards = total_shards,
                "Storing object with {} chunks ({} total shards using {}/{} erasure coding)",
                chunk_count,
                total_shards,
                DATA_SHARDS,
                PARITY_SHARDS
            );

            // Create file record FIRST so chunks can reference it (foreign key)
            let create_file = cyxcloud_metadata::CreateFile {
                id: Some(file_id),
                name: key.split('/').last().unwrap_or(key).to_string(),
                path: format!("{}/{}", bucket, key),
                content_hash: content_hash.as_bytes().to_vec(),
                size_bytes: data.len() as i64,
                chunk_count: chunk_count as i32,
                data_shards: DATA_SHARDS as i32,
                parity_shards: PARITY_SHARDS as i32,
                chunk_size: DEFAULT_CHUNK_SIZE as i32,
                owner_id: Some(self.user_id),
                bucket: Some(bucket.to_string()),
                content_type: Some(content_type.to_string()),
                metadata: None,
            };
            let file = meta
                .register_file(create_file)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            debug!(file_id = %file.id, "File record created, now storing shards");

            // Track total shards stored for verification
            let mut shards_stored = 0;
            let mut failed_shards = 0;

            // Process each chunk with erasure coding
            for chunk in &chunks {
                let chunk_id = chunk.metadata.id.as_bytes();

                // Encode chunk into shards using erasure coding
                // For large chunks (> 1MB), use parallel encoding
                let shards = if chunk.data.len() > 1024 * 1024 {
                    erasure_encoder.encode_parallel(&chunk.data)
                } else {
                    erasure_encoder.encode(&chunk.data)
                }
                .map_err(|e| S3Error::Internal(format!("Erasure encoding failed: {}", e)))?;

                debug!(
                    chunk_index = chunk.metadata.index,
                    chunk_size = chunk.data.len(),
                    shard_count = shards.len(),
                    "Encoded chunk into {} shards",
                    shards.len()
                );

                // Use PlacementEngine to select nodes for shard distribution
                // Each shard needs 1 replica (erasure coding provides redundancy)
                let placement_decisions = placement_engine.select_nodes(
                    &placement_nodes,
                    shards.len(), // Number of shards to place
                    1,            // 1 replica per shard (erasure coding handles redundancy)
                    None,         // No origin preference
                );

                // Distribute shards to selected nodes
                for (shard, decision) in shards.iter().zip(placement_decisions.iter()) {
                    if decision.nodes.is_empty() {
                        warn!(
                            shard_index = shard.index,
                            "No nodes available for shard, skipping"
                        );
                        failed_shards += 1;
                        continue;
                    }

                    // Create shard-specific chunk ID by hashing the shard data
                    // This satisfies content-addressing: shard_id = hash(shard_data)
                    // which the storage node validates before storing
                    let shard_id = ContentHash::compute(&shard.data).as_bytes().to_vec();

                    // Create metadata for this shard
                    let shard_meta = ChunkMeta {
                        size: shard.data.len() as u64,
                        index: chunk.metadata.index,
                        total_chunks: chunk.metadata.total_chunks,
                        parent_id: chunk.metadata.parent_id,
                        created_at: chunk.metadata.created_at,
                        encrypted: chunk.metadata.encrypted,
                        shard_index: Some(shard.index as u32),
                    };

                    // Get target node address
                    let target_node = &decision.nodes[0];

                    // Store shard on the selected node
                    match self
                        .node_client
                        .store_chunk(
                            &target_node.grpc_address,
                            &shard_id,
                            shard.data.clone(),
                            Some(shard_meta.clone()),
                        )
                        .await
                    {
                        Ok(()) => {
                            debug!(
                                chunk_index = chunk.metadata.index,
                                shard_index = shard.index,
                                node = %target_node.grpc_address,
                                is_parity = shard.is_parity,
                                "Shard stored successfully"
                            );

                            // Register chunk in chunks table
                            let create_chunk = CreateChunk {
                                chunk_id: shard_id.clone(),
                                file_id,
                                chunk_index: chunk.metadata.index as i32,
                                shard_index: shard.index as i32,
                                is_parity: shard.is_parity,
                                size_bytes: shard.data.len() as i32,
                                replication_factor: 3, // Target replicas for rebalancer
                            };
                            if let Err(e) = meta.register_chunk(create_chunk).await {
                                warn!(error = %e, "Failed to register chunk in database");
                            }

                            // Record shard location in metadata
                            if let Some(node) = nodes
                                .iter()
                                .find(|n| n.grpc_address == target_node.grpc_address)
                            {
                                if let Err(e) = meta.record_chunk_location(&shard_id, node.id).await
                                {
                                    warn!(error = %e, "Failed to record shard location");
                                }
                            }
                            shards_stored += 1;
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                chunk_index = chunk.metadata.index,
                                shard_index = shard.index,
                                "Failed to store shard on primary node, trying backup"
                            );

                            // Try to store on any other available node
                            let mut stored = false;
                            for backup_node in placement_nodes.iter() {
                                if backup_node.grpc_address == target_node.grpc_address {
                                    continue;
                                }

                                if let Ok(()) = self
                                    .node_client
                                    .store_chunk(
                                        &backup_node.grpc_address,
                                        &shard_id,
                                        shard.data.clone(),
                                        Some(shard_meta.clone()),
                                    )
                                    .await
                                {
                                    // Register chunk in chunks table
                                    let create_chunk = CreateChunk {
                                        chunk_id: shard_id.clone(),
                                        file_id,
                                        chunk_index: chunk.metadata.index as i32,
                                        shard_index: shard.index as i32,
                                        is_parity: shard.is_parity,
                                        size_bytes: shard.data.len() as i32,
                                        replication_factor: 3, // Target replicas for rebalancer
                                    };
                                    if let Err(e) = meta.register_chunk(create_chunk).await {
                                        warn!(error = %e, "Failed to register chunk in database (backup node)");
                                    }

                                    if let Some(node) = nodes
                                        .iter()
                                        .find(|n| n.grpc_address == backup_node.grpc_address)
                                    {
                                        if let Err(e) = meta.record_chunk_location(&shard_id, node.id).await {
                                            warn!(error = %e, "Failed to record shard location (backup node)");
                                        }
                                    }
                                    shards_stored += 1;
                                    stored = true;
                                    break;
                                }
                            }

                            if !stored {
                                failed_shards += 1;
                            }
                        }
                    }
                }
            }

            // Check if we stored enough shards (need at least DATA_SHARDS per chunk)
            let min_shards_needed = chunk_count * DATA_SHARDS;
            if shards_stored < min_shards_needed {
                error!(
                    shards_stored = shards_stored,
                    min_needed = min_shards_needed,
                    failed = failed_shards,
                    "Insufficient shards stored, data may not be recoverable"
                );
                return Err(S3Error::Internal(format!(
                    "Failed to store sufficient shards: {} stored, {} needed",
                    shards_stored, min_shards_needed
                )));
            }

            // Calculate ETag
            let etag = hex::encode(content_hash.as_bytes());

            info!(
                bucket = bucket,
                key = key,
                file_id = %file.id,
                etag = %etag,
                shards_stored = shards_stored,
                failed_shards = failed_shards,
                "Object stored successfully with erasure coding"
            );

            // Publish event
            self.publish_file_created(bucket, key, data.len() as u64)
                .await;

            return Ok(etag);
        }

        Err(S3Error::Internal(
            "No storage backend available".to_string(),
        ))
    }

    /// Get an object
    pub async fn get_object(&self, bucket: &str, key: &str) -> S3Result<Bytes> {
        if self.use_memory {
            let buckets = self.memory_buckets.read().await;
            let bucket_state = buckets
                .get(bucket)
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            let obj = bucket_state
                .objects
                .get(key)
                .ok_or_else(|| S3Error::NoSuchKey(key.to_string()))?;

            return Ok(obj.data.clone());
        }

        // Use metadata service + node retrieval with erasure decoding
        if let Some(ref meta) = self.metadata {
            // Get file info from database
            let file_path = format!("{}/{}", bucket, key);
            let file = meta
                .get_file_by_path(&file_path)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?
                .ok_or_else(|| S3Error::NoSuchKey(key.to_string()))?;

            // Get all shard records for this file
            let shard_records = meta
                .get_file_chunks(file.id)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            if shard_records.is_empty() {
                return Err(S3Error::Internal("No shards found for file".to_string()));
            }

            let num_chunks = file.chunk_count as usize;
            info!(
                bucket = bucket,
                key = key,
                file_id = %file.id,
                shards = shard_records.len(),
                chunks = num_chunks,
                "Retrieving object with erasure decoding"
            );

            // Group shard records by chunk_index
            let mut chunk_shards: HashMap<i32, Vec<&cyxcloud_metadata::Chunk>> = HashMap::new();
            for shard in &shard_records {
                chunk_shards
                    .entry(shard.chunk_index)
                    .or_default()
                    .push(shard);
            }

            // Batch-fetch all chunk locations for this file (avoids N+1 queries)
            let all_locations = meta
                .get_file_chunk_locations(file.id)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            // Create erasure decoder
            let erasure_decoder = ErasureEncoder::new().map_err(|e| {
                S3Error::Internal(format!("Failed to create erasure decoder: {}", e))
            })?;

            // Decode each chunk using erasure coding
            let mut decoded_chunks: Vec<(i32, Bytes)> = Vec::with_capacity(num_chunks);

            for chunk_idx in 0..num_chunks as i32 {
                let shards = chunk_shards.get(&chunk_idx).ok_or_else(|| {
                    S3Error::Internal(format!("No shards found for chunk {}", chunk_idx))
                })?;

                // Retrieve shards from storage nodes
                // We need at least DATA_SHARDS (10) out of TOTAL_SHARDS (14)
                let mut shard_opts: Vec<Option<ShardData>> = vec![None; TOTAL_SHARDS];
                let mut retrieved_count = 0;

                for shard_record in shards {
                    if retrieved_count >= DATA_SHARDS {
                        // We have enough shards, no need to retrieve more
                        break;
                    }

                    let shard_idx = shard_record.shard_index as usize;
                    if shard_idx >= TOTAL_SHARDS {
                        warn!(shard_index = shard_idx, "Invalid shard index, skipping");
                        continue;
                    }

                    // Look up node addresses from batch-fetched map
                    let addresses = all_locations
                        .get(&shard_record.chunk_id)
                        .cloned()
                        .unwrap_or_default();

                    if addresses.is_empty() {
                        debug!(
                            chunk_index = chunk_idx,
                            shard_index = shard_idx,
                            "No nodes have this shard, will try to reconstruct"
                        );
                        continue;
                    }

                    // Retrieve shard from any available node
                    match self
                        .node_client
                        .get_chunk_from_any(&addresses, &shard_record.chunk_id)
                        .await
                    {
                        Ok(data) => {
                            debug!(
                                chunk_index = chunk_idx,
                                shard_index = shard_idx,
                                size = data.len(),
                                "Shard retrieved"
                            );
                            shard_opts[shard_idx] = Some(ShardData::new(
                                shard_idx as u8,
                                data,
                                shard_record.is_parity,
                            ));
                            retrieved_count += 1;
                        }
                        Err(e) => {
                            debug!(
                                error = %e,
                                chunk_index = chunk_idx,
                                shard_index = shard_idx,
                                "Failed to retrieve shard, will try to reconstruct"
                            );
                        }
                    }
                }

                // Check if we have enough shards to decode
                if retrieved_count < DATA_SHARDS {
                    error!(
                        chunk_index = chunk_idx,
                        retrieved = retrieved_count,
                        required = DATA_SHARDS,
                        "Insufficient shards for erasure decoding"
                    );
                    return Err(S3Error::Internal(format!(
                        "Insufficient shards for chunk {}: have {}, need {}",
                        chunk_idx, retrieved_count, DATA_SHARDS
                    )));
                }

                // Calculate the original chunk size for this chunk
                // For the last chunk, it may be smaller
                let chunk_size = if chunk_idx == (num_chunks as i32 - 1) {
                    // Last chunk: remaining bytes
                    let full_chunks_size =
                        (num_chunks - 1) * file.chunk_size as usize;
                    file.size_bytes as usize - full_chunks_size
                } else {
                    file.chunk_size as usize
                };

                // Decode shards back to original chunk data
                let decoded = erasure_decoder
                    .decode(&shard_opts, chunk_size)
                    .map_err(|e| {
                        S3Error::Internal(format!(
                            "Erasure decoding failed for chunk {}: {}",
                            chunk_idx, e
                        ))
                    })?;

                debug!(
                    chunk_index = chunk_idx,
                    decoded_size = decoded.len(),
                    "Chunk decoded successfully"
                );
                decoded_chunks.push((chunk_idx, decoded));
            }

            // Sort by chunk index and concatenate
            decoded_chunks.sort_by_key(|(idx, _)| *idx);

            let mut result = Vec::with_capacity(file.size_bytes as usize);
            for (_, chunk_data) in decoded_chunks {
                result.extend_from_slice(&chunk_data);
            }

            // Trim to exact original size (should already be correct, but be safe)
            result.truncate(file.size_bytes as usize);

            info!(
                bucket = bucket,
                key = key,
                size = result.len(),
                "Object retrieved and decoded successfully"
            );

            return Ok(Bytes::from(result));
        }

        Err(S3Error::NoSuchKey(key.to_string()))
    }

    /// Get object range
    pub async fn get_object_range(
        &self,
        bucket: &str,
        key: &str,
        start: u64,
        end: u64,
    ) -> S3Result<Bytes> {
        let data = self.get_object(bucket, key).await?;
        let start = start as usize;
        let end = (end as usize + 1).min(data.len());

        if start >= data.len() {
            return Err(S3Error::InvalidRequest("Range out of bounds".to_string()));
        }

        Ok(data.slice(start..end))
    }

    /// Get object with content hash verification
    ///
    /// Returns the object data only if the content hash matches the expected hash.
    /// Used by DataStream service to ensure data integrity during training.
    pub async fn get_object_verified(
        &self,
        bucket: &str,
        key: &str,
        expected_hash: &[u8],
    ) -> S3Result<Bytes> {
        let data = self.get_object(bucket, key).await?;

        // Compute actual hash
        let actual_hash = ContentHash::compute(&data);

        // Verify hash matches
        if actual_hash.as_bytes() != expected_hash {
            warn!(
                bucket = bucket,
                key = key,
                expected = hex::encode(expected_hash),
                actual = actual_hash.to_hex(),
                "Content hash verification failed"
            );
            return Err(S3Error::Internal(format!(
                "Content hash mismatch for {}/{}: expected {}, got {}",
                bucket,
                key,
                hex::encode(expected_hash),
                actual_hash.to_hex()
            )));
        }

        debug!(
            bucket = bucket,
            key = key,
            hash = actual_hash.to_hex(),
            "Object retrieved with verified hash"
        );

        Ok(data)
    }

    /// Delete an object
    pub async fn delete_object(&self, bucket: &str, key: &str) -> S3Result<()> {
        if self.use_memory {
            let mut buckets = self.memory_buckets.write().await;
            let bucket_state = buckets
                .get_mut(bucket)
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            if let Some(removed) = bucket_state.objects.remove(key) {
                self.memory_bytes_used
                    .fetch_sub(removed.data.len(), std::sync::atomic::Ordering::Relaxed);
            }

            // Publish event
            drop(buckets);
            self.publish_file_deleted(bucket, key).await;

            return Ok(());
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            // Get file info from database
            let file_path = format!("{}/{}", bucket, key);
            let file = meta
                .get_file_by_path(&file_path)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            if let Some(file) = file {
                // Delete the file (soft delete)
                meta.delete_file(file.id)
                    .await
                    .map_err(|e| S3Error::Internal(e.to_string()))?;

                info!(bucket = bucket, key = key, file_id = %file.id, "Object deleted (database)");

                // Publish event
                self.publish_file_deleted(bucket, key).await;
            }
            // If file doesn't exist, that's okay for DELETE

            return Ok(());
        }

        Err(S3Error::Internal(
            "No storage backend available".to_string(),
        ))
    }

    /// Get object metadata
    pub async fn get_object_metadata(
        &self,
        bucket: &str,
        key: &str,
    ) -> S3Result<Option<ObjectMetadata>> {
        if self.use_memory {
            let buckets = self.memory_buckets.read().await;
            let bucket_state = buckets
                .get(bucket)
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            let obj = match bucket_state.objects.get(key) {
                Some(o) => o,
                None => return Ok(None),
            };

            return Ok(Some(ObjectMetadata {
                key: key.to_string(),
                size: obj.data.len() as u64,
                content_type: obj.content_type.clone(),
                etag: obj.etag.clone(),
                last_modified: obj.created_at.to_rfc3339(),
            }));
        }

        // Use metadata service
        if let Some(ref meta) = self.metadata {
            // Get file info from database
            let file_path = format!("{}/{}", bucket, key);
            let file = meta
                .get_file_by_path(&file_path)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            if let Some(file) = file {
                return Ok(Some(ObjectMetadata {
                    key: key.to_string(),
                    size: file.size_bytes as u64,
                    content_type: file
                        .content_type
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    etag: hex::encode(&file.content_hash),
                    last_modified: file.updated_at.to_rfc3339(),
                }));
            }

            return Ok(None);
        }

        Ok(None)
    }

    /// List objects in bucket
    pub async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
        _delimiter: Option<&str>,
        max_keys: i32,
        _continuation_token: Option<&str>,
    ) -> S3Result<(Vec<ObjectInfo>, bool, Option<String>)> {
        if self.use_memory {
            let buckets = self.memory_buckets.read().await;
            let bucket_state = buckets
                .get(bucket)
                .ok_or_else(|| S3Error::NoSuchBucket(bucket.to_string()))?;

            let mut objects: Vec<_> = bucket_state
                .objects
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, v)| ObjectInfo {
                    key: k.clone(),
                    last_modified: v.created_at.to_rfc3339(),
                    etag: v.etag.clone(),
                    size: v.data.len() as u64,
                    storage_class: "STANDARD".to_string(),
                })
                .collect();

            objects.sort_by(|a, b| a.key.cmp(&b.key));

            let is_truncated = objects.len() > max_keys as usize;
            let objects: Vec<_> = objects.into_iter().take(max_keys as usize).collect();

            return Ok((objects, is_truncated, None));
        }

        // Use metadata service for file listing
        if let Some(ref meta) = self.metadata {
            let db = meta.database();
            let files = db
                .list_files_in_bucket(bucket, Some(prefix), max_keys as i64, 0)
                .await
                .map_err(|e| S3Error::Internal(e.to_string()))?;

            let objects: Vec<ObjectInfo> = files
                .into_iter()
                .map(|f| ObjectInfo {
                    key: f.path.clone(),
                    last_modified: f.created_at.to_rfc3339(),
                    etag: hex::encode(&f.content_hash),
                    size: f.size_bytes as u64,
                    storage_class: "STANDARD".to_string(),
                })
                .collect();

            let is_truncated = objects.len() >= max_keys as usize;
            return Ok((objects, is_truncated, None));
        }

        Ok((Vec::new(), false, None))
    }

    // =========================================================================
    // GRPC DATA SERVICE OPERATIONS
    // =========================================================================

    /// Get dataset info
    pub async fn get_dataset(
        &self,
        dataset_id: &str,
    ) -> Result<
        Option<crate::grpc_api::data_service::DatasetInfo>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        if let Some(ref meta) = self.metadata {
            // Try to parse as UUID
            if let Ok(uuid) = Uuid::parse_str(dataset_id) {
                if let Ok(Some(file)) = meta.get_file(uuid).await {
                    return Ok(Some(crate::grpc_api::data_service::DatasetInfo {
                        id: file.id.to_string(),
                        name: file.name.clone(),
                        size_bytes: file.size_bytes as u64,
                        num_samples: 0, // Would need to parse from metadata
                        num_chunks: file.chunk_count as u32,
                        sample_shape: vec![],
                        dtype: "float32".to_string(),
                        metadata_json: file.metadata.map(|m| m.to_string()).unwrap_or_default(),
                    }));
                }
            }
        }

        // Return placeholder for testing
        Ok(Some(crate::grpc_api::data_service::DatasetInfo {
            id: dataset_id.to_string(),
            name: "Placeholder Dataset".to_string(),
            size_bytes: 0,
            num_samples: 0,
            num_chunks: 0,
            sample_shape: vec![],
            dtype: "float32".to_string(),
            metadata_json: "{}".to_string(),
        }))
    }

    /// Get dataset chunks
    pub async fn get_dataset_chunks(
        &self,
        dataset_id: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ref meta) = self.metadata {
            if let Ok(uuid) = Uuid::parse_str(dataset_id) {
                let chunks = meta.get_file_chunks(uuid).await?;
                return Ok(chunks.iter().map(|c| hex::encode(&c.chunk_id)).collect());
            }
        }
        Ok(Vec::new())
    }

    /// Get chunk data
    pub async fn get_chunk_data(
        &self,
        chunk_id: &str,
    ) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ref meta) = self.metadata {
            // Get chunk locations
            let chunk_bytes = hex::decode(chunk_id)?;
            let locations = meta.get_chunk_locations(&chunk_bytes).await?;

            if locations.is_empty() {
                return Err("No nodes have this chunk".into());
            }

            debug!(chunk_id = chunk_id, nodes = ?locations, "Fetching chunk from storage nodes");

            // Retrieve chunk from any available node
            match self
                .node_client
                .get_chunk_from_any(&locations, &chunk_bytes)
                .await
            {
                Ok(data) => {
                    debug!(
                        chunk_id = chunk_id,
                        size = data.len(),
                        "Chunk retrieved successfully"
                    );
                    return Ok(data);
                }
                Err(e) => {
                    error!(chunk_id = chunk_id, error = %e, "Failed to retrieve chunk from any node");
                    return Err(format!("Failed to retrieve chunk: {}", e).into());
                }
            }
        }

        Err("Metadata service not available".into())
    }

    /// Prefetch chunk
    pub async fn prefetch_chunk(
        &self,
        _dataset_id: &str,
        _chunk_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // TODO: Implement caching/prefetching
        Ok(())
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
