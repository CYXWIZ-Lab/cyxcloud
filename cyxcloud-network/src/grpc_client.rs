//! gRPC client for communicating with other CyxCloud nodes
//!
//! Provides connection pooling, retry logic, and high-level chunk operations.

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::{CyxCloudError, Result};
use cyxcloud_core::tls::{create_tonic_client_tls, TlsClientConfig};
use cyxcloud_protocol::chunk::{
    chunk_service_client::ChunkServiceClient, DeleteChunkRequest, GetChunkRequest,
    StoreChunkRequest, StreamChunksRequest, VerifyChunkRequest,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;
use tracing::{debug, info, instrument, warn};

/// Configuration for the gRPC client
#[derive(Debug, Clone)]
pub struct ChunkClientConfig {
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Maximum retries for failed operations
    pub max_retries: u32,
    /// Initial retry delay (doubles with each retry)
    pub retry_delay: Duration,
    /// Maximum message size in bytes
    pub max_message_size: usize,
    /// Keep-alive interval
    pub keep_alive_interval: Duration,
    /// Enable TLS for connections
    pub enable_tls: bool,
    /// CA certificate path for TLS
    pub tls_ca_cert: Option<PathBuf>,
    /// Client certificate path for mTLS
    pub tls_client_cert: Option<PathBuf>,
    /// Client key path for mTLS
    pub tls_client_key: Option<PathBuf>,
}

impl Default for ChunkClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
            max_message_size: 64 * 1024 * 1024, // 64 MB
            keep_alive_interval: Duration::from_secs(60),
            enable_tls: false,
            tls_ca_cert: None,
            tls_client_cert: None,
            tls_client_key: None,
        }
    }
}

/// Client for communicating with CyxCloud nodes via gRPC
pub struct ChunkClient {
    /// Connection pool: address -> client
    clients: Arc<RwLock<HashMap<String, ChunkServiceClient<Channel>>>>,
    /// Configuration
    config: ChunkClientConfig,
}

impl ChunkClient {
    /// Create a new ChunkClient with default configuration
    pub fn new() -> Self {
        Self::with_config(ChunkClientConfig::default())
    }

    /// Create a new ChunkClient with custom configuration
    pub fn with_config(config: ChunkClientConfig) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Get or create a client for the given address
    async fn get_client(&self, addr: &str) -> Result<ChunkServiceClient<Channel>> {
        // Check if we have an existing connection
        {
            let clients = self.clients.read();
            if let Some(client) = clients.get(addr) {
                return Ok(client.clone());
            }
        }

        // Determine endpoint scheme based on TLS config
        let endpoint_url = if self.config.enable_tls {
            format!("https://{}", addr)
        } else {
            format!("http://{}", addr)
        };
        debug!(addr = %addr, tls = self.config.enable_tls, "Creating new gRPC connection");

        let mut endpoint = Channel::from_shared(endpoint_url.clone())
            .map_err(|e| CyxCloudError::Network(format!("Invalid endpoint: {}", e)))?
            .connect_timeout(self.config.connect_timeout)
            .timeout(self.config.request_timeout)
            .http2_keep_alive_interval(self.config.keep_alive_interval)
            .keep_alive_timeout(Duration::from_secs(20));

        // Configure TLS if enabled
        if self.config.enable_tls {
            if let Some(ref ca_cert_path) = self.config.tls_ca_cert {
                let tls_config = TlsClientConfig {
                    ca_cert_path: ca_cert_path.clone(),
                    client_cert_path: self.config.tls_client_cert.clone(),
                    client_key_path: self.config.tls_client_key.clone(),
                };

                let tls = create_tonic_client_tls(&tls_config).map_err(|e| {
                    CyxCloudError::Network(format!("Failed to load TLS config: {}", e))
                })?;
                endpoint = endpoint.tls_config(tls).map_err(|e| {
                    CyxCloudError::Network(format!("Failed to configure TLS: {}", e))
                })?;

                debug!(
                    addr = %addr,
                    mtls = self.config.tls_client_cert.is_some(),
                    "TLS configured for inter-node connection"
                );
            }
        }

        let channel = endpoint
            .connect()
            .await
            .map_err(|e| CyxCloudError::Network(format!("Connection failed to {}: {}", addr, e)))?;

        let client = ChunkServiceClient::new(channel)
            .max_decoding_message_size(self.config.max_message_size)
            .max_encoding_message_size(self.config.max_message_size);

        // Cache the connection
        {
            let mut clients = self.clients.write();
            clients.insert(addr.to_string(), client.clone());
        }

        info!(addr = %addr, tls = self.config.enable_tls, "gRPC connection established");
        Ok(client)
    }

    /// Remove a cached connection (e.g., after failure)
    fn remove_client(&self, addr: &str) {
        let mut clients = self.clients.write();
        clients.remove(addr);
        debug!(addr = %addr, "Removed gRPC connection from cache");
    }

    /// Execute an operation with retry logic
    async fn with_retry<F, Fut, T>(&self, addr: &str, operation: F) -> Result<T>
    where
        F: Fn(ChunkServiceClient<Channel>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_error = None;
        let mut delay = self.config.retry_delay;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                warn!(
                    addr = %addr,
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying operation"
                );
                tokio::time::sleep(delay).await;
                delay *= 2; // Exponential backoff

                // Remove cached connection before retry
                self.remove_client(addr);
            }

            match self.get_client(addr).await {
                Ok(client) => match operation(client).await {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        warn!(addr = %addr, attempt = attempt, error = %e, "Operation failed");
                        last_error = Some(e);
                    }
                },
                Err(e) => {
                    warn!(addr = %addr, attempt = attempt, error = %e, "Connection failed");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            CyxCloudError::Network("Operation failed with unknown error".to_string())
        }))
    }

    /// Store a chunk on a remote node
    #[instrument(skip(self, data), fields(addr = %addr, chunk_id = %chunk_id))]
    pub async fn store_chunk(&self, addr: &str, chunk_id: ChunkId, data: Bytes) -> Result<()> {
        debug!(size = data.len(), "Storing chunk on remote node");

        self.with_retry(addr, |mut client| {
            let chunk_id = chunk_id;
            let data = data.clone();
            async move {
                let request = tonic::Request::new(StoreChunkRequest {
                    chunk_id: chunk_id.as_bytes().to_vec(),
                    data: data.to_vec(),
                    metadata: None,
                });

                let response = client
                    .store_chunk(request)
                    .await
                    .map_err(|e| CyxCloudError::Network(format!("StoreChunk RPC failed: {}", e)))?;

                let inner = response.into_inner();
                if inner.success {
                    Ok(())
                } else {
                    Err(CyxCloudError::Network(format!(
                        "StoreChunk failed: {}",
                        inner.error
                    )))
                }
            }
        })
        .await
    }

    /// Get a chunk from a remote node
    #[instrument(skip(self), fields(addr = %addr, chunk_id = %chunk_id))]
    pub async fn get_chunk(&self, addr: &str, chunk_id: ChunkId) -> Result<Option<Bytes>> {
        debug!("Getting chunk from remote node");

        self.with_retry(addr, |mut client| {
            let chunk_id = chunk_id;
            async move {
                let request = tonic::Request::new(GetChunkRequest {
                    chunk_id: chunk_id.as_bytes().to_vec(),
                });

                let response = client
                    .get_chunk(request)
                    .await
                    .map_err(|e| CyxCloudError::Network(format!("GetChunk RPC failed: {}", e)))?;

                let inner = response.into_inner();
                if inner.found {
                    Ok(Some(Bytes::from(inner.data)))
                } else {
                    Ok(None)
                }
            }
        })
        .await
    }

    /// Delete a chunk from a remote node
    #[instrument(skip(self), fields(addr = %addr, chunk_id = %chunk_id))]
    pub async fn delete_chunk(&self, addr: &str, chunk_id: ChunkId) -> Result<bool> {
        debug!("Deleting chunk from remote node");

        self.with_retry(addr, |mut client| {
            let chunk_id = chunk_id;
            async move {
                let request = tonic::Request::new(DeleteChunkRequest {
                    chunk_id: chunk_id.as_bytes().to_vec(),
                });

                let response = client.delete_chunk(request).await.map_err(|e| {
                    CyxCloudError::Network(format!("DeleteChunk RPC failed: {}", e))
                })?;

                Ok(response.into_inner().deleted)
            }
        })
        .await
    }

    /// Verify a chunk on a remote node
    #[instrument(skip(self), fields(addr = %addr, chunk_id = %chunk_id))]
    pub async fn verify_chunk(&self, addr: &str, chunk_id: ChunkId) -> Result<(bool, u64)> {
        debug!("Verifying chunk on remote node");

        self.with_retry(addr, |mut client| {
            let chunk_id = chunk_id;
            async move {
                let request = tonic::Request::new(VerifyChunkRequest {
                    chunk_id: chunk_id.as_bytes().to_vec(),
                });

                let response = client.verify_chunk(request).await.map_err(|e| {
                    CyxCloudError::Network(format!("VerifyChunk RPC failed: {}", e))
                })?;

                let inner = response.into_inner();
                Ok((inner.valid, inner.size))
            }
        })
        .await
    }

    /// Stream multiple chunks from a remote node
    #[instrument(skip(self, chunk_ids), fields(addr = %addr, count = chunk_ids.len()))]
    pub async fn stream_chunks(
        &self,
        addr: &str,
        chunk_ids: Vec<ChunkId>,
    ) -> Result<Vec<(ChunkId, Bytes)>> {
        debug!("Streaming chunks from remote node");

        self.with_retry(addr, |mut client| {
            let chunk_ids = chunk_ids.clone();
            async move {
                let request = tonic::Request::new(StreamChunksRequest {
                    chunk_ids: chunk_ids.iter().map(|id| id.as_bytes().to_vec()).collect(),
                });

                let mut stream = client
                    .stream_chunks(request)
                    .await
                    .map_err(|e| CyxCloudError::Network(format!("StreamChunks RPC failed: {}", e)))?
                    .into_inner();

                let mut results = Vec::new();

                while let Some(chunk_data) = stream
                    .message()
                    .await
                    .map_err(|e| CyxCloudError::Network(format!("Stream error: {}", e)))?
                {
                    if chunk_data.chunk_id.len() == 32 {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&chunk_data.chunk_id);
                        let id = ChunkId::from_bytes(arr);
                        results.push((id, Bytes::from(chunk_data.data)));
                    }
                }

                Ok(results)
            }
        })
        .await
    }

    /// Check if a node is reachable by attempting to connect
    pub async fn is_reachable(&self, addr: &str) -> bool {
        match self.get_client(addr).await {
            Ok(_) => true,
            Err(e) => {
                debug!(addr = %addr, error = %e, "Node not reachable");
                false
            }
        }
    }

    /// Clear all cached connections
    pub fn clear_connections(&self) {
        let mut clients = self.clients.write();
        let count = clients.len();
        clients.clear();
        info!(count = count, "Cleared all cached connections");
    }

    /// Get the number of cached connections
    pub fn connection_count(&self) -> usize {
        self.clients.read().len()
    }
}

impl Default for ChunkClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Store a chunk to multiple nodes, returning list of successful nodes
pub async fn store_to_multiple_nodes(
    client: &ChunkClient,
    chunk_id: ChunkId,
    data: Bytes,
    nodes: &[String],
) -> Result<Vec<String>> {
    let mut successful = Vec::new();
    let mut errors = Vec::new();

    // Store to all nodes concurrently
    let futures: Vec<_> = nodes
        .iter()
        .map(|addr| {
            let client = client;
            let addr = addr.clone();
            let data = data.clone();
            async move {
                (
                    addr.clone(),
                    client.store_chunk(&addr, chunk_id, data).await,
                )
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    for (addr, result) in results {
        match result {
            Ok(()) => successful.push(addr),
            Err(e) => {
                warn!(addr = %addr, error = %e, "Failed to store chunk");
                errors.push((addr, e));
            }
        }
    }

    if successful.is_empty() && !errors.is_empty() {
        return Err(CyxCloudError::Network(format!(
            "Failed to store chunk to any node: {:?}",
            errors
                .iter()
                .map(|(a, e)| format!("{}: {}", a, e))
                .collect::<Vec<_>>()
        )));
    }

    Ok(successful)
}

/// Get a chunk from any of the provided nodes (tries until success)
pub async fn get_from_any_node(
    client: &ChunkClient,
    chunk_id: ChunkId,
    nodes: &[String],
) -> Result<Bytes> {
    for addr in nodes {
        match client.get_chunk(addr, chunk_id).await {
            Ok(Some(data)) => return Ok(data),
            Ok(None) => {
                debug!(addr = %addr, chunk_id = %chunk_id, "Chunk not found on node");
            }
            Err(e) => {
                warn!(addr = %addr, chunk_id = %chunk_id, error = %e, "Failed to get chunk");
            }
        }
    }

    Err(CyxCloudError::ChunkNotFound(chunk_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ChunkClientConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.max_message_size, 64 * 1024 * 1024);
    }

    #[test]
    fn test_client_creation() {
        let client = ChunkClient::new();
        assert_eq!(client.connection_count(), 0);
    }

    #[test]
    fn test_clear_connections() {
        let client = ChunkClient::new();
        client.clear_connections();
        assert_eq!(client.connection_count(), 0);
    }
}
