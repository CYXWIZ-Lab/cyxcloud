//! Error types for CyxCloud
//!
//! Provides a unified error type for all CyxCloud operations.

use thiserror::Error;

/// Result type alias for CyxCloud operations
pub type Result<T> = std::result::Result<T, CyxCloudError>;

/// Unified error type for CyxCloud
#[derive(Error, Debug)]
pub enum CyxCloudError {
    // ===== Erasure Coding Errors =====
    #[error("Erasure coding error: {0}")]
    ErasureCoding(String),

    #[error("Insufficient shards: have {available}, need {required}")]
    InsufficientShards { available: usize, required: usize },

    #[error("Shard size mismatch: expected {expected}, got {actual}")]
    ShardSizeMismatch { expected: usize, actual: usize },

    #[error("Invalid shard index: {index} (max: {max})")]
    InvalidShardIndex { index: usize, max: usize },

    // ===== Cryptography Errors =====
    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("Invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },

    #[error("Hash verification failed")]
    HashVerificationFailed,

    // ===== Chunk Errors =====
    #[error("Chunk too large: {size} bytes (max: {max})")]
    ChunkTooLarge { size: usize, max: usize },

    #[error("Chunk too small: {size} bytes (min: {min})")]
    ChunkTooSmall { size: usize, min: usize },

    #[error("Invalid chunk ID: {0}")]
    InvalidChunkId(String),

    #[error("Chunk not found: {0}")]
    ChunkNotFound(String),

    #[error("Chunk corrupted: CID mismatch")]
    ChunkCorrupted,

    // ===== Storage Errors =====
    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Storage full: {used} / {capacity} bytes")]
    StorageFull { used: u64, capacity: u64 },

    // ===== Network Errors =====
    #[error("Network error: {0}")]
    Network(String),

    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    #[error("Connection timeout to {peer}")]
    ConnectionTimeout { peer: String },

    #[error("Quorum not met: {achieved}/{required}")]
    QuorumNotMet { achieved: usize, required: usize },

    // ===== I/O Errors =====
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    // ===== Serialization Errors =====
    #[error("Serialization error: {0}")]
    Serialization(String),

    // ===== Configuration Errors =====
    #[error("Configuration error: {0}")]
    Configuration(String),

    // ===== Generic Errors =====
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<reed_solomon_erasure::Error> for CyxCloudError {
    fn from(err: reed_solomon_erasure::Error) -> Self {
        CyxCloudError::ErasureCoding(err.to_string())
    }
}

impl From<bincode::Error> for CyxCloudError {
    fn from(err: bincode::Error) -> Self {
        CyxCloudError::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CyxCloudError::InsufficientShards {
            available: 8,
            required: 10,
        };
        assert_eq!(
            err.to_string(),
            "Insufficient shards: have 8, need 10"
        );
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        );
        let err: CyxCloudError = io_err.into();
        assert!(matches!(err, CyxCloudError::Io(_)));
    }
}
