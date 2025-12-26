//! gRPC ChunkService implementation
//!
//! Implements the ChunkService for storing and retrieving data chunks.
//! This is the server-side implementation that handles incoming requests
//! from other nodes in the CyxCloud network.

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_protocol::chunk::{
    chunk_service_server::ChunkService, ChunkData, DeleteChunkRequest, DeleteChunkResponse,
    GetChunkRequest, GetChunkResponse, StoreChunkRequest, StoreChunkResponse,
    StreamChunksRequest, VerifyChunkRequest, VerifyChunkResponse,
};
use cyxcloud_storage::backend::StorageBackendSync;
use cyxcloud_storage::RocksDbBackend;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, instrument, warn};

/// Configuration for the gRPC server
#[derive(Debug, Clone)]
pub struct GrpcServerConfig {
    /// Address to listen on
    pub listen_addr: SocketAddr,
    /// Maximum message size in bytes (default 64 MB for large chunks)
    pub max_message_size: usize,
    /// Enable TLS
    pub enable_tls: bool,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:50051".parse().unwrap(),
            max_message_size: 64 * 1024 * 1024, // 64 MB
            enable_tls: false,
        }
    }
}

impl GrpcServerConfig {
    /// Create a new config with the specified address
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            listen_addr: addr,
            ..Default::default()
        }
    }

    /// Set the listen address
    pub fn with_addr(mut self, addr: SocketAddr) -> Self {
        self.listen_addr = addr;
        self
    }

    /// Enable or disable TLS
    pub fn with_tls(mut self, enable: bool) -> Self {
        self.enable_tls = enable;
        self
    }
}

/// ChunkService implementation using RocksDB storage
pub struct ChunkServiceImpl {
    /// Local chunk storage
    storage: Arc<RocksDbBackend>,
    /// Node ID for logging
    node_id: String,
}

impl ChunkServiceImpl {
    /// Create a new ChunkService with the given storage backend
    pub fn new(storage: Arc<RocksDbBackend>, node_id: String) -> Self {
        Self { storage, node_id }
    }

    /// Convert bytes to ChunkId
    fn bytes_to_chunk_id(bytes: &[u8]) -> Result<ChunkId, Status> {
        if bytes.len() != 32 {
            return Err(Status::invalid_argument(format!(
                "Invalid chunk ID length: expected 32, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Ok(ChunkId::from_bytes(arr))
    }

    /// Convert ChunkId to bytes
    fn chunk_id_to_bytes(id: ChunkId) -> Vec<u8> {
        id.as_bytes().to_vec()
    }
}

#[tonic::async_trait]
impl ChunkService for ChunkServiceImpl {
    /// Store a chunk
    #[instrument(skip(self, request), fields(node_id = %self.node_id))]
    async fn store_chunk(
        &self,
        request: Request<StoreChunkRequest>,
    ) -> Result<Response<StoreChunkResponse>, Status> {
        let req = request.into_inner();
        let chunk_id = Self::bytes_to_chunk_id(&req.chunk_id)?;
        let data_len = req.data.len();

        debug!(chunk_id = %chunk_id, size = data_len, "Storing chunk");

        // Validate chunk data
        if req.data.is_empty() {
            return Err(Status::invalid_argument("Chunk data cannot be empty"));
        }

        // Verify content hash matches chunk ID (content-addressing)
        let computed_id = ChunkId::from_data(&req.data);
        if computed_id != chunk_id {
            warn!(
                expected = %chunk_id,
                computed = %computed_id,
                "Chunk ID mismatch - data doesn't match claimed ID"
            );
            return Err(Status::invalid_argument(
                "Chunk ID doesn't match data hash",
            ));
        }

        // Store the chunk
        match self.storage.put(chunk_id, Bytes::from(req.data)) {
            Ok(()) => {
                info!(chunk_id = %chunk_id, size = data_len, "Chunk stored successfully");
                Ok(Response::new(StoreChunkResponse {
                    success: true,
                    error: String::new(),
                }))
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to store chunk");
                Ok(Response::new(StoreChunkResponse {
                    success: false,
                    error: e.to_string(),
                }))
            }
        }
    }

    /// Retrieve a chunk
    #[instrument(skip(self, request), fields(node_id = %self.node_id))]
    async fn get_chunk(
        &self,
        request: Request<GetChunkRequest>,
    ) -> Result<Response<GetChunkResponse>, Status> {
        let req = request.into_inner();
        let chunk_id = Self::bytes_to_chunk_id(&req.chunk_id)?;

        debug!(chunk_id = %chunk_id, "Retrieving chunk");

        match self.storage.get(chunk_id) {
            Ok(Some(data)) => {
                debug!(chunk_id = %chunk_id, size = data.len(), "Chunk found");
                Ok(Response::new(GetChunkResponse {
                    data: data.to_vec(),
                    metadata: None, // TODO: Store and retrieve metadata
                    found: true,
                }))
            }
            Ok(None) => {
                debug!(chunk_id = %chunk_id, "Chunk not found");
                Ok(Response::new(GetChunkResponse {
                    data: Vec::new(),
                    metadata: None,
                    found: false,
                }))
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to retrieve chunk");
                Err(Status::internal(format!("Storage error: {}", e)))
            }
        }
    }

    /// Delete a chunk
    #[instrument(skip(self, request), fields(node_id = %self.node_id))]
    async fn delete_chunk(
        &self,
        request: Request<DeleteChunkRequest>,
    ) -> Result<Response<DeleteChunkResponse>, Status> {
        let req = request.into_inner();
        let chunk_id = Self::bytes_to_chunk_id(&req.chunk_id)?;

        debug!(chunk_id = %chunk_id, "Deleting chunk");

        match self.storage.delete(chunk_id) {
            Ok(deleted) => {
                if deleted {
                    info!(chunk_id = %chunk_id, "Chunk deleted");
                } else {
                    debug!(chunk_id = %chunk_id, "Chunk not found for deletion");
                }
                Ok(Response::new(DeleteChunkResponse { deleted }))
            }
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to delete chunk");
                Err(Status::internal(format!("Storage error: {}", e)))
            }
        }
    }

    type StreamChunksStream = ReceiverStream<Result<ChunkData, Status>>;

    /// Stream multiple chunks (server-side streaming)
    #[instrument(skip(self, request), fields(node_id = %self.node_id))]
    async fn stream_chunks(
        &self,
        request: Request<StreamChunksRequest>,
    ) -> Result<Response<Self::StreamChunksStream>, Status> {
        let req = request.into_inner();
        let chunk_ids: Vec<ChunkId> = req
            .chunk_ids
            .iter()
            .map(|bytes| Self::bytes_to_chunk_id(bytes))
            .collect::<Result<Vec<_>, _>>()?;

        info!(count = chunk_ids.len(), "Streaming chunks");

        let (tx, rx) = mpsc::channel(32);
        let storage = self.storage.clone();

        // Spawn task to stream chunks
        tokio::spawn(async move {
            for (index, chunk_id) in chunk_ids.into_iter().enumerate() {
                let result = match storage.get(chunk_id) {
                    Ok(Some(data)) => Ok(ChunkData {
                        chunk_id: Self::chunk_id_to_bytes(chunk_id),
                        data: data.to_vec(),
                        index: index as u32,
                    }),
                    Ok(None) => {
                        // Skip missing chunks but continue streaming
                        warn!(chunk_id = %chunk_id, "Chunk not found during streaming");
                        continue;
                    }
                    Err(e) => {
                        error!(chunk_id = %chunk_id, error = %e, "Error retrieving chunk");
                        Err(Status::internal(format!("Storage error: {}", e)))
                    }
                };

                if tx.send(result).await.is_err() {
                    // Client disconnected
                    debug!("Client disconnected during streaming");
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// Verify chunk integrity
    #[instrument(skip(self, request), fields(node_id = %self.node_id))]
    async fn verify_chunk(
        &self,
        request: Request<VerifyChunkRequest>,
    ) -> Result<Response<VerifyChunkResponse>, Status> {
        let req = request.into_inner();
        let chunk_id = Self::bytes_to_chunk_id(&req.chunk_id)?;

        debug!(chunk_id = %chunk_id, "Verifying chunk");

        match self.storage.get(chunk_id) {
            Ok(Some(data)) => {
                // Verify content hash
                let computed_id = ChunkId::from_data(&data);
                let valid = computed_id == chunk_id;

                if !valid {
                    warn!(
                        chunk_id = %chunk_id,
                        computed = %computed_id,
                        "Chunk verification failed - data corrupted"
                    );
                }

                Ok(Response::new(VerifyChunkResponse {
                    valid,
                    size: data.len() as u64,
                }))
            }
            Ok(None) => Ok(Response::new(VerifyChunkResponse {
                valid: false,
                size: 0,
            })),
            Err(e) => {
                error!(chunk_id = %chunk_id, error = %e, "Failed to verify chunk");
                Err(Status::internal(format!("Storage error: {}", e)))
            }
        }
    }
}

/// Start the gRPC server
pub async fn start_server(
    config: GrpcServerConfig,
    storage: Arc<RocksDbBackend>,
    node_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use cyxcloud_protocol::chunk::chunk_service_server::ChunkServiceServer;

    let service = ChunkServiceImpl::new(storage, node_id.clone());
    let server = ChunkServiceServer::new(service)
        .max_decoding_message_size(config.max_message_size)
        .max_encoding_message_size(config.max_message_size);

    info!(addr = %config.listen_addr, node_id = %node_id, "Starting gRPC server");

    tonic::transport::Server::builder()
        .add_service(server)
        .serve(config.listen_addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyxcloud_storage::StorageConfig;
    use tempfile::TempDir;

    fn create_test_storage() -> (Arc<RocksDbBackend>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let storage = Arc::new(RocksDbBackend::open(config).unwrap());
        (storage, temp_dir)
    }

    #[tokio::test]
    async fn test_store_and_get_chunk() {
        let (storage, _dir) = create_test_storage();
        let service = ChunkServiceImpl::new(storage, "test-node".to_string());

        // Create test data
        let data = b"hello world";
        let chunk_id = ChunkId::from_data(data);

        // Store chunk
        let store_request = Request::new(StoreChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
            data: data.to_vec(),
            metadata: None,
        });

        let store_response = service.store_chunk(store_request).await.unwrap();
        assert!(store_response.into_inner().success);

        // Get chunk
        let get_request = Request::new(GetChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
        });

        let get_response = service.get_chunk(get_request).await.unwrap();
        let inner = get_response.into_inner();
        assert!(inner.found);
        assert_eq!(inner.data, data.to_vec());
    }

    #[tokio::test]
    async fn test_chunk_id_mismatch() {
        let (storage, _dir) = create_test_storage();
        let service = ChunkServiceImpl::new(storage, "test-node".to_string());

        // Create a request with mismatched ID and data
        let data = b"hello world";
        let wrong_id = ChunkId::from_data(b"different data");

        let store_request = Request::new(StoreChunkRequest {
            chunk_id: wrong_id.as_bytes().to_vec(),
            data: data.to_vec(),
            metadata: None,
        });

        let result = service.store_chunk(store_request).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("doesn't match data hash"));
    }

    #[tokio::test]
    async fn test_verify_chunk() {
        let (storage, _dir) = create_test_storage();
        let service = ChunkServiceImpl::new(storage, "test-node".to_string());

        let data = b"verification test";
        let chunk_id = ChunkId::from_data(data);

        // Store chunk first
        let store_request = Request::new(StoreChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
            data: data.to_vec(),
            metadata: None,
        });
        service.store_chunk(store_request).await.unwrap();

        // Verify chunk
        let verify_request = Request::new(VerifyChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
        });

        let verify_response = service.verify_chunk(verify_request).await.unwrap();
        let inner = verify_response.into_inner();
        assert!(inner.valid);
        assert_eq!(inner.size, data.len() as u64);
    }

    #[tokio::test]
    async fn test_delete_chunk() {
        let (storage, _dir) = create_test_storage();
        let service = ChunkServiceImpl::new(storage, "test-node".to_string());

        let data = b"delete me";
        let chunk_id = ChunkId::from_data(data);

        // Store chunk
        let store_request = Request::new(StoreChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
            data: data.to_vec(),
            metadata: None,
        });
        service.store_chunk(store_request).await.unwrap();

        // Delete chunk
        let delete_request = Request::new(DeleteChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
        });

        let delete_response = service.delete_chunk(delete_request).await.unwrap();
        assert!(delete_response.into_inner().deleted);

        // Verify deleted
        let get_request = Request::new(GetChunkRequest {
            chunk_id: chunk_id.as_bytes().to_vec(),
        });

        let get_response = service.get_chunk(get_request).await.unwrap();
        assert!(!get_response.into_inner().found);
    }
}
