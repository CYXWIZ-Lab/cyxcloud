//! Storage backend trait
//!
//! Defines the interface that all storage implementations must follow.

use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::Result;
use std::future::Future;
use std::pin::Pin;

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    /// Total number of chunks stored
    pub chunk_count: u64,

    /// Total bytes used by chunks
    pub bytes_used: u64,

    /// Total storage capacity (0 = unlimited)
    pub bytes_capacity: u64,

    /// Number of read operations
    pub reads: u64,

    /// Number of write operations
    pub writes: u64,

    /// Number of delete operations
    pub deletes: u64,

    /// Average read latency in microseconds
    pub avg_read_latency_us: u64,

    /// Average write latency in microseconds
    pub avg_write_latency_us: u64,
}

impl StorageStats {
    /// Calculate usage percentage
    pub fn usage_percent(&self) -> f64 {
        if self.bytes_capacity == 0 {
            0.0
        } else {
            (self.bytes_used as f64 / self.bytes_capacity as f64) * 100.0
        }
    }

    /// Check if storage is full
    pub fn is_full(&self) -> bool {
        self.bytes_capacity > 0 && self.bytes_used >= self.bytes_capacity
    }

    /// Available space in bytes
    pub fn bytes_available(&self) -> u64 {
        if self.bytes_capacity == 0 {
            u64::MAX
        } else {
            self.bytes_capacity.saturating_sub(self.bytes_used)
        }
    }
}

/// Async storage backend trait
///
/// All storage implementations must be Send + Sync for use in async contexts.
pub trait StorageBackend: Send + Sync {
    /// Store a chunk
    fn put<'a>(
        &'a self,
        id: ChunkId,
        data: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Retrieve a chunk
    fn get<'a>(
        &'a self,
        id: ChunkId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Bytes>>> + Send + 'a>>;

    /// Delete a chunk
    fn delete<'a>(&'a self, id: ChunkId)
        -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>>;

    /// Check if a chunk exists
    fn exists<'a>(&'a self, id: ChunkId)
        -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>>;

    /// Get storage statistics
    fn stats<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<StorageStats>> + Send + 'a>>;

    /// List all chunk IDs
    fn list_chunks<'a>(&'a self)
        -> Pin<Box<dyn Future<Output = Result<Vec<ChunkId>>> + Send + 'a>>;

    /// Flush any pending writes to disk
    fn flush<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// Synchronous storage backend trait (for simpler implementations)
pub trait StorageBackendSync: Send + Sync {
    /// Store a chunk
    fn put(&self, id: ChunkId, data: Bytes) -> Result<()>;

    /// Retrieve a chunk
    fn get(&self, id: ChunkId) -> Result<Option<Bytes>>;

    /// Delete a chunk
    fn delete(&self, id: ChunkId) -> Result<bool>;

    /// Check if a chunk exists
    fn exists(&self, id: ChunkId) -> Result<bool>;

    /// Get storage statistics
    fn stats(&self) -> Result<StorageStats>;

    /// List all chunk IDs
    fn list_chunks(&self) -> Result<Vec<ChunkId>>;

    /// Flush any pending writes
    fn flush(&self) -> Result<()>;
}

/// Wrapper to convert sync backend to async
pub struct AsyncWrapper<T: StorageBackendSync>(pub T);

impl<T: StorageBackendSync + 'static> StorageBackend for AsyncWrapper<T> {
    fn put<'a>(
        &'a self,
        id: ChunkId,
        data: Bytes,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { self.0.put(id, data) })
    }

    fn get<'a>(
        &'a self,
        id: ChunkId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Bytes>>> + Send + 'a>> {
        Box::pin(async move { self.0.get(id) })
    }

    fn delete<'a>(
        &'a self,
        id: ChunkId,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>> {
        Box::pin(async move { self.0.delete(id) })
    }

    fn exists<'a>(
        &'a self,
        id: ChunkId,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>> {
        Box::pin(async move { self.0.exists(id) })
    }

    fn stats<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<StorageStats>> + Send + 'a>> {
        Box::pin(async move { self.0.stats() })
    }

    fn list_chunks<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ChunkId>>> + Send + 'a>> {
        Box::pin(async move { self.0.list_chunks() })
    }

    fn flush<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { self.0.flush() })
    }
}
