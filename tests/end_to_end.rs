//! End-to-end integration tests for CyxCloud
//!
//! Tests the complete pipeline: file → chunks → encode → store → retrieve → decode → file
//!
//! Run with: cargo test --test end_to_end

use bytes::Bytes;
use cyxcloud_core::chunk::{split_into_chunks, reassemble_chunks, Chunk, ChunkId};
use cyxcloud_core::crypto::{encrypt, decrypt, EncryptedData, ContentHash, EncryptionKey};
use cyxcloud_core::erasure::{ErasureConfig, ErasureEncoder};
use cyxcloud_core::DEFAULT_CHUNK_SIZE;
use cyxcloud_storage::memory::MemoryBackend;
use cyxcloud_storage::rocks::RocksDbBackend;
use cyxcloud_storage::backend::StorageBackendSync;
use cyxcloud_storage::StorageConfig;
use std::collections::HashMap;
use tempfile::TempDir;

/// Generate test file data of specified size
fn generate_file(size: usize) -> Vec<u8> {
    // Use a pattern that's easy to verify
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Test the complete pipeline: file → chunks → encode → encrypt → store → retrieve → decrypt → decode → file
#[test]
fn test_full_pipeline_memory_backend() {
    let backend = MemoryBackend::new();
    run_full_pipeline_test(&backend, 1024 * 1024); // 1 MB file
}

#[test]
fn test_full_pipeline_rocksdb() {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();
    run_full_pipeline_test(&backend, 1024 * 1024); // 1 MB file
}

#[test]
fn test_full_pipeline_large_file() {
    let backend = MemoryBackend::new();
    run_full_pipeline_test(&backend, 10 * 1024 * 1024); // 10 MB file
}

fn run_full_pipeline_test<B: StorageBackendSync>(backend: &B, file_size: usize) {
    // 1. Generate test file
    let original_file = generate_file(file_size);
    let original_hash = ContentHash::compute(&original_file);

    // 2. Split into chunks (256 KB chunks for testing)
    let chunk_size = 256 * 1024;
    let chunks = split_into_chunks(&original_file, chunk_size, None).unwrap();
    assert!(!chunks.is_empty());
    println!("Split file into {} chunks", chunks.len());

    // 3. Generate encryption key
    let key = EncryptionKey::generate();

    // 4. For each chunk: encrypt → erasure encode → store
    let encoder = ErasureEncoder::new().unwrap();
    let mut stored_data: HashMap<ChunkId, (Vec<(usize, ChunkId)>, usize)> = HashMap::new();

    for chunk in &chunks {
        // Encrypt the chunk
        let encrypted = encrypt(&chunk.data, &key).unwrap();
        let encrypted_bytes = encrypted.to_bytes();
        let encrypted_size = encrypted_bytes.len();

        // Erasure encode
        let shards = encoder.encode(&encrypted_bytes).unwrap();
        assert_eq!(shards.len(), ErasureConfig::TOTAL_SHARDS);

        // Store each shard
        let mut shard_ids = Vec::new();
        for (shard_idx, shard_data) in shards.into_iter().enumerate() {
            let shard_id = ChunkId::from_data(&[
                chunk.id().as_bytes().as_slice(),
                &(shard_idx as u32).to_le_bytes(),
            ].concat());

            backend.put(shard_id, shard_data).unwrap();
            shard_ids.push((shard_idx, shard_id));
        }

        stored_data.insert(chunk.id(), (shard_ids, encrypted_size));
    }

    println!("Stored {} chunks with erasure coding", stored_data.len());

    // 5. Retrieve: get shards → erasure decode → decrypt → reassemble
    let mut recovered_chunks: Vec<Chunk> = Vec::new();

    for (idx, chunk) in chunks.iter().enumerate() {
        let (shard_ids, encrypted_size) = stored_data.get(&chunk.id()).unwrap();

        // Retrieve all shards
        let mut shard_opts: Vec<Option<Bytes>> = vec![None; ErasureConfig::TOTAL_SHARDS];
        for (shard_idx, shard_id) in shard_ids {
            let shard_data = backend.get(*shard_id).unwrap();
            shard_opts[*shard_idx] = shard_data;
        }

        // Erasure decode
        let decrypted_encoded = encoder.decode(&shard_opts, *encrypted_size).unwrap();

        // Decrypt
        let encrypted = EncryptedData::from_bytes(&decrypted_encoded).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();

        // Create recovered chunk
        let recovered = Chunk::new(
            Bytes::from(decrypted),
            idx as u32,
            chunks.len() as u32,
        ).unwrap();
        recovered_chunks.push(recovered);
    }

    // 6. Reassemble file
    let reconstructed_file = reassemble_chunks(&recovered_chunks).unwrap();

    // 7. Verify
    let reconstructed_hash = ContentHash::compute(&reconstructed_file);
    assert_eq!(
        original_hash, reconstructed_hash,
        "File hash mismatch after reconstruction"
    );
    assert_eq!(
        original_file.as_slice(), reconstructed_file.as_ref(),
        "File content mismatch after reconstruction"
    );

    println!("✓ Full pipeline test passed for {} bytes", file_size);
}

/// Test encryption roundtrip with various sizes
#[test]
fn test_encryption_roundtrip() {
    for size in [1, 16, 256, 1024, 64 * 1024, 1024 * 1024] {
        let data = generate_file(size);
        let key = EncryptionKey::generate();

        // Encrypt
        let encrypted = encrypt(&data, &key).unwrap();

        // Decrypt
        let decrypted = decrypt(&encrypted, &key).unwrap();

        assert_eq!(
            data,
            decrypted,
            "Encryption roundtrip failed for size {}",
            size
        );
    }

    println!("✓ Encryption roundtrip test passed");
}

/// Test partial shard recovery (simulate node failures)
#[test]
fn test_partial_shard_recovery() {
    let encoder = ErasureEncoder::new().unwrap();
    let original_data = generate_file(1024 * 1024); // 1 MB

    // Encode
    let shards = encoder.encode(&original_data).unwrap();
    let original_size = original_data.len();

    // Test recovery with different numbers of missing shards
    for missing_count in 0..=ErasureConfig::PARITY_SHARDS {
        let mut shard_opts: Vec<Option<Bytes>> = shards.iter().cloned().map(Some).collect();

        // Remove some shards
        for i in 0..missing_count {
            shard_opts[i] = None;
        }

        // Decode
        let recovered = encoder.decode(&shard_opts, original_size);
        assert!(
            recovered.is_ok(),
            "Failed to recover with {} missing shards",
            missing_count
        );

        let recovered_data = recovered.unwrap();
        assert_eq!(
            original_data, recovered_data.to_vec(),
            "Data mismatch with {} missing shards",
            missing_count
        );
    }

    println!("✓ Partial shard recovery test passed (0-{} missing)", ErasureConfig::PARITY_SHARDS);
}

/// Test recovery fails with too many missing shards
#[test]
fn test_too_many_missing_shards_fails() {
    let encoder = ErasureEncoder::new().unwrap();
    let original_data = generate_file(64 * 1024); // 64 KB

    let shards = encoder.encode(&original_data).unwrap();
    let original_size = original_data.len();

    // Remove more than allowed parity shards
    let missing_count = ErasureConfig::PARITY_SHARDS + 1;
    let mut shard_opts: Vec<Option<Bytes>> = shards.iter().cloned().map(Some).collect();
    for i in 0..missing_count {
        shard_opts[i] = None;
    }

    let result = encoder.decode(&shard_opts, original_size);
    assert!(
        result.is_err(),
        "Should fail with {} missing shards (max allowed: {})",
        missing_count,
        ErasureConfig::PARITY_SHARDS
    );

    println!("✓ Correctly rejected decode with too many missing shards");
}

/// Test content-addressed storage (same content = same ID)
#[test]
fn test_content_addressing() {
    let data = b"hello world";
    let id1 = ChunkId::from_data(data);
    let id2 = ChunkId::from_data(data);
    let id3 = ChunkId::from_data(b"different data");

    assert_eq!(id1, id2, "Same content should produce same ID");
    assert_ne!(id1, id3, "Different content should produce different ID");

    // Verify base58 encoding works
    let encoded = id1.to_base58();
    assert!(!encoded.is_empty());

    println!("✓ Content addressing test passed");
}

/// Test hash verification
#[test]
fn test_hash_verification() {
    let data = generate_file(1024 * 1024); // 1 MB
    let hash = ContentHash::compute(&data);

    // Verify correct data
    assert!(hash.verify(&data), "Hash should verify correct data");

    // Verify incorrect data fails
    let mut corrupted = data.clone();
    corrupted[500] ^= 0xFF; // Flip some bits
    assert!(!hash.verify(&corrupted), "Hash should reject corrupted data");

    println!("✓ Hash verification test passed");
}

/// Test storage persistence (RocksDB)
#[test]
fn test_storage_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let chunk_id = ChunkId::from_data(b"persistent chunk");
    let data = Bytes::from_static(b"this data should persist");

    // Write and close
    {
        let config = StorageConfig::new(temp_dir.path());
        let backend = RocksDbBackend::open(config).unwrap();
        backend.put(chunk_id, data.clone()).unwrap();
        backend.flush().unwrap();
    }

    // Reopen and verify
    {
        let config = StorageConfig::new(temp_dir.path());
        let backend = RocksDbBackend::open(config).unwrap();
        let retrieved = backend.get(chunk_id).unwrap().unwrap();
        assert_eq!(retrieved, data, "Data should persist after reopen");
    }

    println!("✓ Storage persistence test passed");
}

/// Test parallel encoding
#[test]
fn test_parallel_encoding() {
    let encoder = ErasureEncoder::new().unwrap();
    let data = generate_file(10 * 1024 * 1024); // 10 MB

    // Sequential encode
    let shards_seq = encoder.encode(&data).unwrap();

    // Parallel encode
    let shards_par = encoder.encode_parallel(&data).unwrap();

    // Verify both produce same results
    assert_eq!(shards_seq.len(), shards_par.len());
    for (seq, par) in shards_seq.iter().zip(shards_par.iter()) {
        assert_eq!(seq, par, "Sequential and parallel encoding should match");
    }

    println!("✓ Parallel encoding test passed");
}

/// Test parallel hashing
#[test]
fn test_parallel_hashing() {
    let data = generate_file(50 * 1024 * 1024); // 50 MB

    let hash_seq = ContentHash::compute(&data);
    let hash_par = ContentHash::compute_parallel(&data);

    assert_eq!(
        hash_seq, hash_par,
        "Sequential and parallel hashing should match"
    );

    println!("✓ Parallel hashing test passed");
}

/// Test chunk splitting and reassembly
#[test]
fn test_chunk_split_reassemble() {
    for file_size in [1, 100, 4096, 1024 * 1024, 5 * 1024 * 1024] {
        let original = generate_file(file_size);
        let chunk_size = 256 * 1024; // 256 KB

        let chunks = split_into_chunks(&original, chunk_size, None).unwrap();

        // For non-empty files, verify chunks exist
        assert!(!chunks.is_empty());

        // Reassemble
        let reassembled = reassemble_chunks(&chunks).unwrap();
        assert_eq!(
            original.as_slice(), reassembled.as_ref(),
            "Chunk split/reassemble failed for size {}",
            file_size
        );
    }

    println!("✓ Chunk split/reassemble test passed");
}

/// Test storage statistics
#[test]
fn test_storage_stats() {
    let backend = MemoryBackend::new();

    // Initially empty
    let stats = backend.stats().unwrap();
    assert_eq!(stats.chunk_count, 0);
    assert_eq!(stats.bytes_used, 0);

    // Add some chunks
    for i in 0..10u8 {
        let id = ChunkId::from_data(&[i]);
        let data = Bytes::from(vec![i; 1024]); // 1 KB each
        backend.put(id, data).unwrap();
    }

    let stats = backend.stats().unwrap();
    assert_eq!(stats.chunk_count, 10);
    assert_eq!(stats.bytes_used, 10 * 1024);

    println!("✓ Storage stats test passed");
}

/// Test EncryptedData serialization
#[test]
fn test_encrypted_data_serialization() {
    let key = EncryptionKey::generate();
    let original = b"test data for serialization";

    // Encrypt
    let encrypted = encrypt(original, &key).unwrap();

    // Serialize to bytes
    let bytes = encrypted.to_bytes();

    // Deserialize from bytes
    let restored = EncryptedData::from_bytes(&bytes).unwrap();

    // Decrypt should work
    let decrypted = decrypt(&restored, &key).unwrap();
    assert_eq!(original.as_slice(), decrypted.as_slice());

    println!("✓ EncryptedData serialization test passed");
}
