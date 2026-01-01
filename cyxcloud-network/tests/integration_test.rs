//! Full integration tests for the network layer
//!
//! Tests the complete network flow: store → replicate → retrieve

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_network::{
    grpc_client::{get_from_any_node, store_to_multiple_nodes, ChunkClient},
    grpc_server::{start_server, GrpcServerConfig},
    NetworkConfig, NetworkManager,
};
use cyxcloud_storage::{RocksDbBackend, StorageConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

struct TestNode {
    _temp_dir: TempDir,
    handle: tokio::task::JoinHandle<()>,
    addr: String,
}

impl TestNode {
    async fn start(port: u16) -> Self {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let storage = Arc::new(RocksDbBackend::open(config).unwrap());
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        let grpc_config = GrpcServerConfig::new(addr);
        let node_id = format!("node-{}", port);

        let handle = tokio::spawn(async move {
            if let Err(e) = start_server(grpc_config, storage, node_id).await {
                eprintln!("Server error: {}", e);
            }
        });

        // Wait for server to start
        sleep(Duration::from_millis(100)).await;

        TestNode {
            _temp_dir: temp_dir,
            handle,
            addr: format!("127.0.0.1:{}", port),
        }
    }

    fn stop(&self) {
        self.handle.abort();
    }
}

#[tokio::test]
async fn test_multi_node_replication() {
    // Start 3 test nodes
    let nodes = vec![
        TestNode::start(50200).await,
        TestNode::start(50201).await,
        TestNode::start(50202).await,
    ];

    let client = ChunkClient::new();
    let target_addrs: Vec<String> = nodes.iter().map(|n| n.addr.clone()).collect();

    // Create test data
    let data = b"replicated data across three nodes";
    let chunk_id = ChunkId::from_data(data);

    // Store to all nodes
    let successful =
        store_to_multiple_nodes(&client, chunk_id, Bytes::from_static(data), &target_addrs)
            .await
            .unwrap();

    // All 3 nodes should have received the chunk
    assert_eq!(successful.len(), 3, "Expected 3 successful stores");

    // Verify each node has the chunk
    for addr in &target_addrs {
        let retrieved = client.get_chunk(addr, chunk_id).await.unwrap();
        assert!(retrieved.is_some(), "Node {} should have the chunk", addr);
        assert_eq!(retrieved.unwrap().as_ref(), data);
    }

    // Cleanup
    for node in &nodes {
        node.stop();
    }
}

#[tokio::test]
async fn test_get_from_any_node() {
    // Start 3 test nodes
    let nodes = vec![
        TestNode::start(50210).await,
        TestNode::start(50211).await,
        TestNode::start(50212).await,
    ];

    let client = ChunkClient::new();
    let target_addrs: Vec<String> = nodes.iter().map(|n| n.addr.clone()).collect();

    // Create test data and store to only one node
    let data = b"data on single node";
    let chunk_id = ChunkId::from_data(data);

    // Store only to the second node
    client
        .store_chunk(&nodes[1].addr, chunk_id, Bytes::from_static(data))
        .await
        .unwrap();

    // Get from any node - should find it on the second node
    let retrieved = get_from_any_node(&client, chunk_id, &target_addrs)
        .await
        .unwrap();

    assert_eq!(retrieved.as_ref(), data);

    // Cleanup
    for node in &nodes {
        node.stop();
    }
}

#[tokio::test]
async fn test_get_from_any_node_failure() {
    // Start 2 nodes but don't store the chunk on any
    let nodes = vec![TestNode::start(50220).await, TestNode::start(50221).await];

    let client = ChunkClient::new();
    let target_addrs: Vec<String> = nodes.iter().map(|n| n.addr.clone()).collect();

    // Try to get a chunk that doesn't exist anywhere
    let chunk_id = ChunkId::from_data(b"nonexistent data");

    let result = get_from_any_node(&client, chunk_id, &target_addrs).await;
    assert!(result.is_err(), "Expected error for nonexistent chunk");

    // Cleanup
    for node in &nodes {
        node.stop();
    }
}

#[tokio::test]
async fn test_partial_replication_failure() {
    // Start only 2 nodes but try to replicate to 3 (one address will fail)
    let nodes = vec![TestNode::start(50230).await, TestNode::start(50231).await];

    let client = ChunkClient::new();

    // Include an address that doesn't exist
    let target_addrs = vec![
        nodes[0].addr.clone(),
        nodes[1].addr.clone(),
        "127.0.0.1:59999".to_string(), // This node doesn't exist
    ];

    let data = b"partial replication test";
    let chunk_id = ChunkId::from_data(data);

    // Store should succeed for 2 nodes, fail for 1
    let successful =
        store_to_multiple_nodes(&client, chunk_id, Bytes::from_static(data), &target_addrs)
            .await
            .unwrap();

    // Only 2 nodes should have received the chunk
    assert_eq!(successful.len(), 2, "Expected 2 successful stores");

    // Verify the successful nodes have the chunk
    for addr in &successful {
        let retrieved = client.get_chunk(addr, chunk_id).await.unwrap();
        assert!(retrieved.is_some());
    }

    // Cleanup
    for node in &nodes {
        node.stop();
    }
}

#[tokio::test]
async fn test_network_manager_creation() {
    let temp_dir = TempDir::new().unwrap();
    let storage_config = StorageConfig::new(temp_dir.path());
    let storage = Arc::new(RocksDbBackend::open(storage_config).unwrap());

    let network_config = NetworkConfig::new("test-manager-node");
    let manager = NetworkManager::new(network_config, storage);

    assert_eq!(manager.node_id(), "test-manager-node");
    assert_eq!(manager.connection_count(), 0);
}

#[tokio::test]
async fn test_network_manager_with_custom_address() {
    let temp_dir = TempDir::new().unwrap();
    let storage_config = StorageConfig::new(temp_dir.path());
    let storage = Arc::new(RocksDbBackend::open(storage_config).unwrap());

    let addr: SocketAddr = "127.0.0.1:60000".parse().unwrap();
    let network_config = NetworkConfig::new("custom-addr-node").with_grpc_addr(addr);
    let manager = NetworkManager::new(network_config, storage);

    assert_eq!(manager.node_id(), "custom-addr-node");
}

#[tokio::test]
async fn test_large_chunk_transfer() {
    // Start a test node
    let node = TestNode::start(50240).await;

    let client = ChunkClient::new();

    // Create a larger chunk (1 MB)
    let data = vec![0xAB_u8; 1024 * 1024];
    let chunk_id = ChunkId::from_data(&data);

    // Store the large chunk
    let result = client
        .store_chunk(&node.addr, chunk_id, Bytes::from(data.clone()))
        .await;
    assert!(result.is_ok(), "Large chunk store failed: {:?}", result);

    // Retrieve and verify
    let retrieved = client.get_chunk(&node.addr, chunk_id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().len(), data.len());

    // Cleanup
    node.stop();
}

#[tokio::test]
async fn test_concurrent_operations() {
    // Start a test node
    let node = TestNode::start(50250).await;

    let client = Arc::new(ChunkClient::new());
    let addr = node.addr.clone();

    // Spawn multiple concurrent store operations
    let mut handles = Vec::new();

    for i in 0..10 {
        let client = client.clone();
        let addr = addr.clone();
        let handle = tokio::spawn(async move {
            let data = format!("concurrent data {}", i);
            let chunk_id = ChunkId::from_data(data.as_bytes());
            client.store_chunk(&addr, chunk_id, Bytes::from(data)).await
        });
        handles.push(handle);
    }

    // Wait for all operations to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // Verify all succeeded
    for result in results {
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }

    // Cleanup
    node.stop();
}

#[tokio::test]
async fn test_node_reachability() {
    let node = TestNode::start(50260).await;
    let client = ChunkClient::new();

    // Running node should be reachable
    assert!(client.is_reachable(&node.addr).await);

    // Non-existent node should not be reachable
    assert!(!client.is_reachable("127.0.0.1:59998").await);

    // Cleanup
    node.stop();
}
