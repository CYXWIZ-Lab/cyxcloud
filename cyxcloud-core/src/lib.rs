//! CyxCloud Core Library
//!
//! Core abstractions for the CyxCloud decentralized storage platform.
//! This crate provides:
//! - Reed-Solomon erasure coding (10 data + 4 parity shards)
//! - Cryptographic primitives (Blake3 hashing, AES-GCM encryption)
//! - Content-addressed chunk identifiers (CID)
//! - Common types and error handling

pub mod chunk;
pub mod crypto;
pub mod erasure;
pub mod error;

pub use chunk::{Chunk, ChunkId, ChunkMetadata, split_into_chunks, reassemble_chunks};
pub use crypto::{ContentHash, EncryptedData, EncryptionKey, encrypt, decrypt};
pub use erasure::{ErasureConfig, ErasureEncoder, ShardData};
pub use error::{CyxCloudError, Result};

/// Default erasure coding configuration
/// - 10 data shards: minimum required to reconstruct
/// - 4 parity shards: can tolerate 4 node failures
/// - 14 total shards distributed across nodes
pub const DATA_SHARDS: usize = 10;
pub const PARITY_SHARDS: usize = 4;
pub const TOTAL_SHARDS: usize = DATA_SHARDS + PARITY_SHARDS;

/// Chunk size constants
pub const MIN_CHUNK_SIZE: usize = 256 * 1024;        // 256 KB
pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4 MB
pub const MAX_CHUNK_SIZE: usize = 64 * 1024 * 1024;   // 64 MB
