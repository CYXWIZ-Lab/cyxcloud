//! DataStream Client for Server Node
//!
//! Provides a gRPC client for streaming ML training data from CyxCloud Gateway
//! with hash verification and batch prefetching.

use cyxcloud_core::tls::{create_tonic_client_tls, TlsClientConfig};
use cyxcloud_protocol::datastream::{
    data_stream_service_client::DataStreamServiceClient, BatchResponse, CreateAccessTokenRequest,
    DatasetInfo, GetDatasetInfoRequest, StreamBatchesRequest, TrustLevel, VerificationResult,
    VerifyDatasetRequest,
};
use thiserror::Error;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use tracing::{debug, error, info, instrument};

/// DataStream client errors
#[derive(Error, Debug)]
pub enum DataStreamError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("gRPC error: {0}")]
    GrpcError(#[from] tonic::Status),

    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("Dataset not found: {0}")]
    DatasetNotFound(String),

    #[error("Access denied")]
    AccessDenied,

    #[error("Token expired")]
    TokenExpired,

    #[error("Hash verification failed: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Trust level {actual} exceeds maximum allowed {max}")]
    TrustLevelExceeded { actual: i32, max: i32 },

    #[error("Stream ended unexpectedly")]
    StreamEnded,
}

pub type DataStreamResult<T> = Result<T, DataStreamError>;

/// Configuration for DataStream client
#[derive(Debug, Clone)]
pub struct DataStreamConfig {
    /// Gateway address (e.g., "http://localhost:50052")
    pub gateway_addr: String,

    /// Access token for authentication
    pub access_token: Option<String>,

    /// Default batch size
    pub batch_size: i32,

    /// Number of batches to prefetch
    pub prefetch_batches: i32,

    /// Maximum acceptable trust level (0=self, 4=untrusted)
    pub max_trust_level: i32,

    /// TLS configuration (optional)
    pub tls_config: Option<TlsClientConfig>,

    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,
}

impl Default for DataStreamConfig {
    fn default() -> Self {
        Self {
            gateway_addr: "http://localhost:50052".to_string(),
            access_token: None,
            batch_size: 32,
            prefetch_batches: 4,
            max_trust_level: TrustLevel::TrustVerified as i32,
            tls_config: None,
            connect_timeout_secs: 30,
        }
    }
}

impl DataStreamConfig {
    /// Create config from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(addr) = std::env::var("GATEWAY_ADDR") {
            config.gateway_addr = addr;
        }

        if let Ok(token) = std::env::var("DATASTREAM_ACCESS_TOKEN") {
            config.access_token = Some(token);
        }

        if let Ok(batch_size) = std::env::var("DATASTREAM_BATCH_SIZE") {
            if let Ok(size) = batch_size.parse() {
                config.batch_size = size;
            }
        }

        if let Ok(prefetch) = std::env::var("DATASTREAM_PREFETCH_BATCHES") {
            if let Ok(n) = prefetch.parse() {
                config.prefetch_batches = n;
            }
        }

        if let Ok(trust) = std::env::var("DATASTREAM_MAX_TRUST_LEVEL") {
            if let Ok(level) = trust.parse() {
                config.max_trust_level = level;
            }
        }

        config
    }
}

/// A verified batch of training data
#[derive(Debug, Clone)]
pub struct VerifiedBatch {
    /// Batch index (0-based)
    pub index: u64,

    /// Raw data items in the batch
    pub items: Vec<Vec<u8>>,

    /// Number of items in the batch
    pub item_count: usize,

    /// Whether this is the last batch
    pub is_last: bool,

    /// Total number of batches in the dataset
    pub total_batches: u64,
}

/// DataStream client for streaming ML training data
pub struct DataStreamClient {
    client: DataStreamServiceClient<Channel>,
    config: DataStreamConfig,
}

impl DataStreamClient {
    /// Create a new DataStream client
    #[instrument(skip(config), fields(gateway = %config.gateway_addr))]
    pub async fn connect(config: DataStreamConfig) -> DataStreamResult<Self> {
        info!("Connecting to DataStream gateway");

        let channel = if let Some(ref tls_config) = config.tls_config {
            // TLS connection
            let tls = create_tonic_client_tls(tls_config)
                .map_err(|e| DataStreamError::ConnectionFailed(e.to_string()))?;

            Channel::from_shared(config.gateway_addr.clone())
                .map_err(|e| DataStreamError::ConnectionFailed(e.to_string()))?
                .tls_config(tls)?
                .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
                .connect()
                .await?
        } else {
            // Plain connection
            Channel::from_shared(config.gateway_addr.clone())
                .map_err(|e| DataStreamError::ConnectionFailed(e.to_string()))?
                .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
                .connect()
                .await?
        };

        let client = DataStreamServiceClient::new(channel);

        info!("Connected to DataStream gateway");

        Ok(Self { client, config })
    }

    /// Get dataset information
    #[instrument(skip(self))]
    pub async fn get_dataset_info(&mut self, dataset_id: &str) -> DataStreamResult<DatasetInfo> {
        let request = GetDatasetInfoRequest {
            dataset_id: dataset_id.to_string(),
        };

        let response = self.client.get_dataset_info(request).await?;
        let info = response.into_inner();

        info
            .dataset
            .ok_or_else(|| DataStreamError::DatasetNotFound(dataset_id.to_string()))
    }

    /// Create an access token for streaming
    #[instrument(skip(self))]
    pub async fn create_access_token(
        &mut self,
        dataset_id: &str,
        node_id: Option<&str>,
        scopes: Vec<String>,
        ttl_seconds: i64,
    ) -> DataStreamResult<String> {
        let request = CreateAccessTokenRequest {
            dataset_id: dataset_id.to_string(),
            node_id: node_id.unwrap_or("").to_string(),
            scopes,
            ttl_seconds,
        };

        let response = self.client.create_access_token(request).await?;
        let token_response = response.into_inner();

        Ok(token_response.token)
    }

    /// Verify a dataset's integrity
    #[instrument(skip(self))]
    pub async fn verify_dataset(
        &mut self,
        dataset_id: &str,
        check_public_registry: bool,
        full_verification: bool,
    ) -> DataStreamResult<VerificationResult> {
        let request = VerifyDatasetRequest {
            dataset_id: dataset_id.to_string(),
            check_public_registry,
            full_verification,
        };

        let response = self.client.verify_dataset(request).await?;
        Ok(response.into_inner())
    }

    /// Stream batches from a dataset
    ///
    /// Returns a channel receiver that yields verified batches.
    /// Batches are prefetched in background for better throughput.
    #[instrument(skip(self))]
    pub async fn stream_batches(
        &mut self,
        dataset_id: &str,
        start_index: i64,
        shuffle: bool,
        seed: Option<i64>,
    ) -> DataStreamResult<mpsc::Receiver<DataStreamResult<VerifiedBatch>>> {
        let request = StreamBatchesRequest {
            dataset_id: dataset_id.to_string(),
            access_token: self.config.access_token.clone().unwrap_or_default(),
            batch_size: self.config.batch_size,
            start_index,
            prefetch_batches: self.config.prefetch_batches,
            max_trust_level: self.config.max_trust_level,
            shuffle,
            seed: seed.unwrap_or(0),
        };

        let response = self.client.stream_batches(request).await?;
        let mut stream = response.into_inner();

        // Create channel for verified batches
        let (tx, rx) = mpsc::channel(self.config.prefetch_batches as usize);
        let max_trust_level = self.config.max_trust_level;

        // Spawn background task to receive and verify batches
        tokio::spawn(async move {
            while let Ok(Some(batch)) = stream.message().await {
                match verify_batch(&batch, max_trust_level) {
                    Ok(verified) => {
                        if tx.send(Ok(verified)).await.is_err() {
                            debug!("Batch receiver dropped, stopping stream");
                            break;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Batch verification failed");
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }

            debug!("Batch stream completed");
        });

        Ok(rx)
    }

    /// Stream all batches from a dataset as an async iterator
    ///
    /// This is a convenience method that yields all batches.
    pub async fn stream_all_batches(
        &mut self,
        dataset_id: &str,
        shuffle: bool,
        seed: Option<i64>,
    ) -> DataStreamResult<BatchIterator> {
        let rx = self.stream_batches(dataset_id, 0, shuffle, seed).await?;
        Ok(BatchIterator { rx })
    }

    /// Set the access token for authentication
    pub fn set_access_token(&mut self, token: String) {
        self.config.access_token = Some(token);
    }

    /// Update batch size
    pub fn set_batch_size(&mut self, batch_size: i32) {
        self.config.batch_size = batch_size;
    }

    /// Update prefetch count
    pub fn set_prefetch_batches(&mut self, prefetch: i32) {
        self.config.prefetch_batches = prefetch;
    }

    /// Get current configuration
    pub fn config(&self) -> &DataStreamConfig {
        &self.config
    }
}

/// Verify a batch response
fn verify_batch(batch: &BatchResponse, _max_trust_level: i32) -> DataStreamResult<VerifiedBatch> {
    // Verify item hashes
    if batch.items.len() != batch.item_hashes.len() {
        return Err(DataStreamError::HashMismatch {
            expected: format!("{} item hashes", batch.items.len()),
            actual: format!("{} hashes provided", batch.item_hashes.len()),
        });
    }

    for (i, (item, expected_hash)) in batch.items.iter().zip(batch.item_hashes.iter()).enumerate() {
        let actual_hash = blake3::hash(item);
        if actual_hash.as_bytes() != expected_hash.as_slice() {
            return Err(DataStreamError::HashMismatch {
                expected: hex::encode(expected_hash),
                actual: hex::encode(actual_hash.as_bytes()),
            });
        }
        debug!(item_index = i, "Item hash verified");
    }

    // Verify batch hash
    let mut hasher = blake3::Hasher::new();
    for item in &batch.items {
        hasher.update(item);
    }
    let computed_batch_hash = hasher.finalize();

    if computed_batch_hash.as_bytes() != batch.batch_hash.as_slice() {
        return Err(DataStreamError::HashMismatch {
            expected: hex::encode(&batch.batch_hash),
            actual: hex::encode(computed_batch_hash.as_bytes()),
        });
    }

    debug!(
        batch_index = batch.batch_index,
        items = batch.items.len(),
        "Batch hash verified"
    );

    Ok(VerifiedBatch {
        index: batch.batch_index,
        items: batch.items.clone(),
        item_count: batch.items.len(),
        is_last: batch.is_last,
        total_batches: batch.total_batches,
    })
}

/// Async iterator over batches
pub struct BatchIterator {
    rx: mpsc::Receiver<DataStreamResult<VerifiedBatch>>,
}

impl BatchIterator {
    /// Get the next batch
    pub async fn next(&mut self) -> Option<DataStreamResult<VerifiedBatch>> {
        self.rx.recv().await
    }

    /// Collect all remaining batches
    pub async fn collect_all(mut self) -> DataStreamResult<Vec<VerifiedBatch>> {
        let mut batches = Vec::new();
        while let Some(result) = self.next().await {
            batches.push(result?);
        }
        Ok(batches)
    }
}

/// Builder for DataStreamClient
pub struct DataStreamClientBuilder {
    config: DataStreamConfig,
}

impl DataStreamClientBuilder {
    /// Create a new builder with default config
    pub fn new() -> Self {
        Self {
            config: DataStreamConfig::default(),
        }
    }

    /// Set gateway address
    pub fn gateway_addr(mut self, addr: impl Into<String>) -> Self {
        self.config.gateway_addr = addr.into();
        self
    }

    /// Set access token
    pub fn access_token(mut self, token: impl Into<String>) -> Self {
        self.config.access_token = Some(token.into());
        self
    }

    /// Set batch size
    pub fn batch_size(mut self, size: i32) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Set prefetch count
    pub fn prefetch_batches(mut self, count: i32) -> Self {
        self.config.prefetch_batches = count;
        self
    }

    /// Set maximum trust level
    pub fn max_trust_level(mut self, level: i32) -> Self {
        self.config.max_trust_level = level;
        self
    }

    /// Set TLS configuration
    pub fn tls_config(mut self, config: TlsClientConfig) -> Self {
        self.config.tls_config = Some(config);
        self
    }

    /// Set connection timeout
    pub fn connect_timeout_secs(mut self, secs: u64) -> Self {
        self.config.connect_timeout_secs = secs;
        self
    }

    /// Build and connect the client
    pub async fn connect(self) -> DataStreamResult<DataStreamClient> {
        DataStreamClient::connect(self.config).await
    }
}

impl Default for DataStreamClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = DataStreamConfig::default();
        assert_eq!(config.batch_size, 32);
        assert_eq!(config.prefetch_batches, 4);
        assert_eq!(config.max_trust_level, TrustLevel::TrustVerified as i32);
    }

    #[test]
    fn test_verify_batch_hash_mismatch() {
        let batch = BatchResponse {
            batch_index: 0,
            items: vec![b"test data".to_vec()],
            item_hashes: vec![vec![0u8; 32]], // Wrong hash
            batch_hash: vec![0u8; 32],
            total_batches: 1,
            is_last: true,
        };

        let result = verify_batch(&batch, 2);
        assert!(matches!(result, Err(DataStreamError::HashMismatch { .. })));
    }

    #[test]
    fn test_verify_batch_success() {
        let item = b"test data".to_vec();
        let item_hash = blake3::hash(&item);

        let mut hasher = blake3::Hasher::new();
        hasher.update(&item);
        let batch_hash = hasher.finalize();

        let batch = BatchResponse {
            batch_index: 0,
            items: vec![item],
            item_hashes: vec![item_hash.as_bytes().to_vec()],
            batch_hash: batch_hash.as_bytes().to_vec(),
            total_batches: 1,
            is_last: true,
        };

        let result = verify_batch(&batch, 2);
        assert!(result.is_ok());

        let verified = result.unwrap();
        assert_eq!(verified.index, 0);
        assert_eq!(verified.item_count, 1);
        assert!(verified.is_last);
    }

    #[test]
    fn test_builder() {
        let builder = DataStreamClientBuilder::new()
            .gateway_addr("http://example.com:50052")
            .access_token("test-token")
            .batch_size(64)
            .prefetch_batches(8)
            .max_trust_level(1);

        assert_eq!(builder.config.gateway_addr, "http://example.com:50052");
        assert_eq!(builder.config.access_token, Some("test-token".to_string()));
        assert_eq!(builder.config.batch_size, 64);
        assert_eq!(builder.config.prefetch_batches, 8);
        assert_eq!(builder.config.max_trust_level, 1);
    }
}
