//! CyxCloud Core Library
//!
//! Core abstractions for the CyxCloud distributed storage platform.
//! This crate provides:
//! - Reed-Solomon erasure coding (10 data + 4 parity shards)
//! - Cryptographic primitives (Blake3 hashing, AES-GCM encryption)
//! - Content-addressed chunk identifiers (CID)
//! - Common types and error handling

pub mod chunk;
pub mod crypto;
pub mod erasure;
pub mod error;
pub mod tls;

pub use chunk::{reassemble_chunks, split_into_chunks, Chunk, ChunkId, ChunkMetadata};
pub use crypto::{decrypt, encrypt, ContentHash, EncryptedData, EncryptionKey};
pub use erasure::{ErasureConfig, ErasureEncoder, ShardData};
pub use error::{CyxCloudError, Result};

/// Default erasure coding configuration
/// - 10 data shards: minimum required to reconstruct
/// - 4 parity shards: can tolerate 4 node failures
/// - 14 total shards distributed across nodes
///
/// Override at runtime via ERASURE_DATA_SHARDS / ERASURE_PARITY_SHARDS env vars.
pub const DATA_SHARDS: usize = 10;
pub const PARITY_SHARDS: usize = 4;
pub const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

/// Read erasure shard counts from environment, falling back to compile-time defaults.
/// Returns (data_shards, parity_shards, total_shards).
pub fn erasure_config_from_env() -> (usize, usize, usize) {
    let data = std::env::var("ERASURE_DATA_SHARDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DATA_SHARDS);
    let parity = std::env::var("ERASURE_PARITY_SHARDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(PARITY_SHARDS);
    (data, parity, data + parity)
}

/// Chunk size constants
pub const MIN_CHUNK_SIZE: usize = 256 * 1024; // 256 KB
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB
pub const MAX_CHUNK_SIZE: usize = 64 * 1024 * 1024; // 64 MB
