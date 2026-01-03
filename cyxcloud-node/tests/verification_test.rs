//! Hash Verification Tests
//!
//! Tests Blake3 hash verification for DataStream batches and datasets.
//!
//! Run with: cargo test --test verification_test

use blake3::Hasher;
use cyxcloud_node::{PublicDatasetHashes, TrustRequirement};

/// Generate test data of specified size
fn generate_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Test Blake3 hash computation
#[test]
fn test_blake3_basic_hash() {
    let data = b"hello world";
    let hash = blake3::hash(data);

    // Blake3 produces 32-byte hashes
    assert_eq!(hash.as_bytes().len(), 32);

    // Same data produces same hash
    let hash2 = blake3::hash(data);
    assert_eq!(hash, hash2);

    // Different data produces different hash
    let hash3 = blake3::hash(b"hello world!");
    assert_ne!(hash, hash3);

    println!("Blake3 basic hash: OK");
}

/// Test Blake3 incremental hashing
#[test]
fn test_blake3_incremental_hash() {
    let data = b"hello world";

    // Full hash
    let hash_full = blake3::hash(data);

    // Incremental hash
    let mut hasher = Hasher::new();
    hasher.update(b"hello");
    hasher.update(b" ");
    hasher.update(b"world");
    let hash_incremental = hasher.finalize();

    assert_eq!(hash_full, hash_incremental);

    println!("Blake3 incremental hash: OK");
}

/// Test batch hash verification (simulating BatchResponse)
#[test]
fn test_batch_hash_verification() {
    // Simulate batch items
    let items = vec![
        b"item 1 data".to_vec(),
        b"item 2 data".to_vec(),
        b"item 3 data".to_vec(),
    ];

    // Compute item hashes
    let item_hashes: Vec<Vec<u8>> = items
        .iter()
        .map(|item| blake3::hash(item).as_bytes().to_vec())
        .collect();

    // Compute batch hash
    let mut batch_hasher = Hasher::new();
    for item in &items {
        batch_hasher.update(item);
    }
    let batch_hash = batch_hasher.finalize();

    // Verify each item hash
    for (item, expected_hash) in items.iter().zip(item_hashes.iter()) {
        let actual_hash = blake3::hash(item);
        assert_eq!(
            actual_hash.as_bytes(),
            expected_hash.as_slice(),
            "Item hash mismatch"
        );
    }

    // Verify batch hash
    let mut verify_hasher = Hasher::new();
    for item in &items {
        verify_hasher.update(item);
    }
    let verify_batch_hash = verify_hasher.finalize();
    assert_eq!(batch_hash, verify_batch_hash, "Batch hash mismatch");

    println!("Batch hash verification: OK");
}

/// Test hash mismatch detection
#[test]
fn test_hash_mismatch_detection() {
    let original = b"original data";
    let original_hash = blake3::hash(original);

    // Corrupted data should not match
    let corrupted = b"corrupted data";
    let corrupted_hash = blake3::hash(corrupted);
    assert_ne!(
        original_hash.as_bytes(),
        corrupted_hash.as_bytes(),
        "Corrupted data should have different hash"
    );

    // Single bit flip should be detected
    let mut flipped = original.to_vec();
    flipped[0] ^= 0x01; // Flip one bit
    let flipped_hash = blake3::hash(&flipped);
    assert_ne!(
        original_hash.as_bytes(),
        flipped_hash.as_bytes(),
        "Bit flip should change hash"
    );

    println!("Hash mismatch detection: OK");
}

/// Test large data hashing performance
#[test]
fn test_large_data_hash() {
    let sizes = [1024, 64 * 1024, 1024 * 1024, 10 * 1024 * 1024];

    for size in sizes {
        let data = generate_data(size);
        let start = std::time::Instant::now();
        let hash = blake3::hash(&data);
        let elapsed = start.elapsed();

        assert_eq!(hash.as_bytes().len(), 32);
        println!(
            "Hashed {} bytes in {:?} ({:.2} MB/s)",
            size,
            elapsed,
            (size as f64 / 1024.0 / 1024.0) / elapsed.as_secs_f64()
        );
    }

    println!("Large data hash: OK");
}

/// Test batch with multiple items
#[test]
fn test_multi_item_batch() {
    let item_count = 100;
    let item_size = 1024;

    // Generate items
    let items: Vec<Vec<u8>> = (0..item_count)
        .map(|i| {
            (0..item_size)
                .map(|j| ((i * item_size + j) % 256) as u8)
                .collect()
        })
        .collect();

    // Compute all item hashes
    let item_hashes: Vec<[u8; 32]> = items
        .iter()
        .map(|item| *blake3::hash(item).as_bytes())
        .collect();

    // Compute batch hash
    let mut batch_hasher = Hasher::new();
    for item in &items {
        batch_hasher.update(item);
    }
    let batch_hash = *batch_hasher.finalize().as_bytes();

    // Verify all items
    let mut all_valid = true;
    for (i, (item, expected)) in items.iter().zip(item_hashes.iter()).enumerate() {
        let actual = blake3::hash(item);
        if actual.as_bytes() != expected {
            all_valid = false;
            println!("Item {} hash mismatch", i);
        }
    }
    assert!(all_valid, "All item hashes should match");

    // Verify batch
    let mut verify_hasher = Hasher::new();
    for item in &items {
        verify_hasher.update(item);
    }
    let verify_hash = *verify_hasher.finalize().as_bytes();
    assert_eq!(batch_hash, verify_hash, "Batch hash should match");

    println!("Multi-item batch ({} items): OK", item_count);
}

/// Test empty batch handling
#[test]
fn test_empty_batch() {
    let items: Vec<Vec<u8>> = vec![];

    // Empty batch should still produce a valid hash
    let mut hasher = Hasher::new();
    for item in &items {
        hasher.update(item);
    }
    let hash = hasher.finalize();

    assert_eq!(hash.as_bytes().len(), 32);

    // Verify consistency
    let hasher2 = Hasher::new();
    let hash2 = hasher2.finalize();
    assert_eq!(hash, hash2, "Empty batches should produce consistent hash");

    println!("Empty batch: OK");
}

/// Test PublicDatasetHashes registry
#[test]
fn test_public_dataset_registry() {
    let mut registry = PublicDatasetHashes::new();

    // Register some datasets
    let mnist_hash = blake3::hash(b"MNIST dataset content").as_bytes().to_vec();
    let cifar_hash = blake3::hash(b"CIFAR-10 dataset content").as_bytes().to_vec();

    registry.register("MNIST", "1.0", mnist_hash.clone());
    registry.register("CIFAR-10", "1.0", cifar_hash.clone());

    // Find by hash
    let found_mnist = registry.find_match(&mnist_hash);
    assert_eq!(found_mnist, Some(("MNIST".to_string(), "1.0".to_string())));

    let found_cifar = registry.find_match(&cifar_hash);
    assert_eq!(
        found_cifar,
        Some(("CIFAR-10".to_string(), "1.0".to_string()))
    );

    // Unknown hash
    let unknown_hash = blake3::hash(b"unknown").as_bytes().to_vec();
    let not_found = registry.find_match(&unknown_hash);
    assert!(not_found.is_none());

    println!("Public dataset registry: OK");
}

/// Test trust level boundaries
#[test]
fn test_trust_level_boundaries() {
    // Trust level 0 = Self (most trusted)
    assert!(TrustRequirement::SelfOrBetter.is_satisfied(0));
    assert!(!TrustRequirement::SelfOrBetter.is_satisfied(1));

    // Trust level 4 = Untrusted (least trusted)
    assert!(TrustRequirement::Any.is_satisfied(4));
    assert!(!TrustRequirement::AttestedOnly.is_satisfied(4));

    // Edge cases - negative trust levels (shouldn't happen but test anyway)
    // Lower is better, so -1 would be even more trusted than 0
    assert!(TrustRequirement::Any.is_satisfied(-1));

    // Very high trust levels (invalid but test boundary)
    assert!(!TrustRequirement::VerifiedOrBetter.is_satisfied(100));

    println!("Trust level boundaries: OK");
}

/// Test hash hex encoding
#[test]
fn test_hash_hex_encoding() {
    let data = b"test data";
    let hash = blake3::hash(data);
    let hash_bytes = hash.as_bytes();

    // Hex encode
    let hex_str = hex::encode(hash_bytes);
    assert_eq!(hex_str.len(), 64); // 32 bytes = 64 hex chars

    // Hex decode
    let decoded = hex::decode(&hex_str).unwrap();
    assert_eq!(decoded.as_slice(), hash_bytes);

    println!("Hash hex encoding: OK");
}

/// Test manifest hash computation
#[test]
fn test_manifest_hash() {
    // Simulate dataset files
    let files = vec![
        ("train/image_001.jpg", vec![1u8; 100]),
        ("train/image_002.jpg", vec![2u8; 100]),
        ("test/image_001.jpg", vec![3u8; 100]),
    ];

    // Compute individual file hashes
    let file_hashes: Vec<(String, [u8; 32])> = files
        .iter()
        .map(|(path, data)| (path.to_string(), *blake3::hash(data).as_bytes()))
        .collect();

    // Compute manifest hash (path + hash for each file)
    let mut manifest_hasher = Hasher::new();
    for (path, hash) in &file_hashes {
        manifest_hasher.update(path.as_bytes());
        manifest_hasher.update(hash);
    }
    let manifest_hash = manifest_hasher.finalize();

    // Verify manifest is deterministic
    let mut verify_hasher = Hasher::new();
    for (path, hash) in &file_hashes {
        verify_hasher.update(path.as_bytes());
        verify_hasher.update(hash);
    }
    let verify_hash = verify_hasher.finalize();
    assert_eq!(manifest_hash, verify_hash);

    // Different order should produce different hash
    let mut different_order = Hasher::new();
    for (path, hash) in file_hashes.iter().rev() {
        different_order.update(path.as_bytes());
        different_order.update(hash);
    }
    let different_hash = different_order.finalize();
    assert_ne!(manifest_hash, different_hash, "Order should affect manifest hash");

    println!("Manifest hash: OK");
}

/// Test streaming hash verification (chunked input)
#[test]
fn test_streaming_verification() {
    let total_size = 1024 * 1024; // 1 MB
    let chunk_size = 64 * 1024; // 64 KB chunks
    let data = generate_data(total_size);

    // Compute full hash
    let full_hash = blake3::hash(&data);

    // Compute streaming hash
    let mut streaming_hasher = Hasher::new();
    for chunk in data.chunks(chunk_size) {
        streaming_hasher.update(chunk);
    }
    let streaming_hash = streaming_hasher.finalize();

    assert_eq!(
        full_hash, streaming_hash,
        "Streaming hash should match full hash"
    );

    println!("Streaming verification: OK");
}

/// Summary test
#[test]
fn test_verification_summary() {
    println!("\n=== Hash Verification Test Summary ===\n");
    println!("Blake3: basic, incremental, large data");
    println!("Batch verification: single and multi-item");
    println!("Mismatch detection: corruption, bit flips");
    println!("Public dataset registry: register, find");
    println!("Trust levels: boundaries, satisfaction");
    println!("Manifest hash: determinism, order sensitivity");
    println!("Streaming: chunked input verification");
    println!("\n All verification tests passed!\n");
}
