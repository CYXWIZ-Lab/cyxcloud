//! CyxCloud Storage Backend
//!
//! Provides storage abstractions and implementations:
//! - `StorageBackend` trait for pluggable storage
//! - `RocksDbBackend` for production chunk storage
//! - `MemoryBackend` for testing
//! - `SledBackend` for metadata storage

pub mod backend;
pub mod memory;
pub mod rocks;
pub mod sled_backend;

pub use backend::{StorageBackend, StorageStats};
pub use memory::MemoryBackend;
pub use rocks::RocksDbBackend;
pub use sled_backend::SledMetadataStore;

/// Storage configuration
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Path to storage directory
    pub path: std::path::PathBuf,

    /// Maximum storage capacity in bytes (0 = unlimited)
    pub max_capacity: u64,

    /// Enable compression for stored chunks
    pub compression: bool,

    /// Cache size in bytes for RocksDB block cache
    pub cache_size: usize,

    /// Number of background compaction threads
    pub compaction_threads: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            path: std::path::PathBuf::from("./cyxcloud_data"),
            max_capacity: 0, // Unlimited
            compression: true,
            cache_size: 512 * 1024 * 1024, // 512 MB
            compaction_threads: 4,
        }
    }
}

impl StorageConfig {
    /// Create a new storage config with the given path
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    /// Set maximum capacity
    pub fn with_max_capacity(mut self, bytes: u64) -> Self {
        self.max_capacity = bytes;
        self
    }

    /// Set cache size
    pub fn with_cache_size(mut self, bytes: usize) -> Self {
        self.cache_size = bytes;
        self
    }

    /// Enable/disable compression
    pub fn with_compression(mut self, enabled: bool) -> Self {
        self.compression = enabled;
        self
    }
}
