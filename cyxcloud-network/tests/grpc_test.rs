//! Integration tests for gRPC ChunkService
//!
//! Tests the full gRPC server and client communication.

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_network::{
    grpc_client::ChunkClient,
    grpc_server::{start_server, GrpcServerConfig},
};
use cyxcloud_storage::{RocksDbBackend, StorageConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

fn create_test_storage() -> (Arc<RocksDbBackend>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let storage = Arc::new(RocksDbBackend::open(config).unwrap());
    (storage, temp_dir)
}

async fn start_test_server(port: u16) -> (TempDir, tokio::task::JoinHandle<()>) {
    let (storage, temp_dir) = create_test_storage();
    let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    let config = GrpcServerConfig::new(addr);
    let node_id = format!("test-node-{}", port);

    let handle = tokio::spawn(async move {
        if let Err(e) = start_server(config, storage, node_id).await {
            eprintln!("Server error: {}", e);
        }
    });

    // Give server time to start
    sleep(Duration::from_millis(100)).await;

    (temp_dir, handle)
}

#[tokio::test]
async fn test_store_and_get_chunk() {
    let port = 50100;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Create test data
    let data = b"hello world from integration test";
    let chunk_id = ChunkId::from_data(data);

    // Store chunk
    let result = client
        .store_chunk(&addr, chunk_id, Bytes::from_static(data))
        .await;
    assert!(result.is_ok(), "Store failed: {:?}", result);

    // Get chunk
    let retrieved = client.get_chunk(&addr, chunk_id).await;
    assert!(retrieved.is_ok(), "Get failed: {:?}", retrieved);

    let retrieved_data = retrieved.unwrap();
    assert!(retrieved_data.is_some(), "Chunk not found");
    assert_eq!(retrieved_data.unwrap().as_ref(), data);

    // Cleanup
    server_handle.abort();
}

#[tokio::test]
async fn test_verify_chunk() {
    let port = 50101;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Create and store test data
    let data = b"verification test data";
    let chunk_id = ChunkId::from_data(data);

    client
        .store_chunk(&addr, chunk_id, Bytes::from_static(data))
        .await
        .unwrap();

    // Verify chunk
    let (valid, size) = client.verify_chunk(&addr, chunk_id).await.unwrap();
    assert!(valid, "Chunk verification failed");
    assert_eq!(size, data.len() as u64);

    // Cleanup
    server_handle.abort();
}

#[tokio::test]
async fn test_delete_chunk() {
    let port = 50102;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Create and store test data
    let data = b"data to be deleted";
    let chunk_id = ChunkId::from_data(data);

    client
        .store_chunk(&addr, chunk_id, Bytes::from_static(data))
        .await
        .unwrap();

    // Verify it exists
    let exists = client.get_chunk(&addr, chunk_id).await.unwrap();
    assert!(exists.is_some());

    // Delete chunk
    let deleted = client.delete_chunk(&addr, chunk_id).await.unwrap();
    assert!(deleted);

    // Verify it's gone
    let gone = client.get_chunk(&addr, chunk_id).await.unwrap();
    assert!(gone.is_none());

    // Cleanup
    server_handle.abort();
}

#[tokio::test]
async fn test_get_nonexistent_chunk() {
    let port = 50103;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Create a chunk ID for data that was never stored
    let chunk_id = ChunkId::from_data(b"never stored");

    // Try to get it
    let result = client.get_chunk(&addr, chunk_id).await.unwrap();
    assert!(result.is_none(), "Expected None for nonexistent chunk");

    // Cleanup
    server_handle.abort();
}

#[tokio::test]
async fn test_stream_chunks() {
    let port = 50104;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Store multiple chunks
    let test_data = [
        b"chunk one data".as_slice(),
        b"chunk two data".as_slice(),
        b"chunk three data".as_slice(),
    ];

    let mut chunk_ids = Vec::new();
    for data in &test_data {
        let chunk_id = ChunkId::from_data(data);
        client
            .store_chunk(&addr, chunk_id, Bytes::copy_from_slice(data))
            .await
            .unwrap();
        chunk_ids.push(chunk_id);
    }

    // Stream all chunks
    let results = client
        .stream_chunks(&addr, chunk_ids.clone())
        .await
        .unwrap();

    assert_eq!(results.len(), 3, "Expected 3 chunks from stream");

    // Verify each chunk
    for (chunk_id, data) in &results {
        assert!(chunk_ids.contains(chunk_id), "Unexpected chunk ID");
        assert!(!data.is_empty(), "Chunk data should not be empty");
    }

    // Cleanup
    server_handle.abort();
}

#[tokio::test]
async fn test_connection_reuse() {
    let port = 50105;
    let (_temp_dir, server_handle) = start_test_server(port).await;

    let client = ChunkClient::new();
    let addr = format!("127.0.0.1:{}", port);

    // Initially no connections
    assert_eq!(client.connection_count(), 0);

    // Make a request to establish connection
    let data = b"connection test";
    let chunk_id = ChunkId::from_data(data);
    client
        .store_chunk(&addr, chunk_id, Bytes::from_static(data))
        .await
        .unwrap();

    // Connection should be cached
    assert_eq!(client.connection_count(), 1);

    // Make another request - should reuse connection
    client.get_chunk(&addr, chunk_id).await.unwrap();
    assert_eq!(client.connection_count(), 1);

    // Clear connections
    client.clear_connections();
    assert_eq!(client.connection_count(), 0);

    // Cleanup
    server_handle.abort();
}
