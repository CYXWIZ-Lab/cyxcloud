//! In-memory storage backend
//!
//! Used for testing and development. Not persistent.

use crate::backend::{StorageBackendSync, StorageStats};
use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::Result;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// In-memory storage backend
pub struct MemoryBackend {
    /// Chunk storage
    chunks: RwLock<HashMap<ChunkId, Bytes>>,

    /// Maximum capacity (0 = unlimited)
    max_capacity: u64,

    /// Current bytes used
    bytes_used: AtomicU64,

    /// Operation counters
    reads: AtomicU64,
    writes: AtomicU64,
    deletes: AtomicU64,
}

impl MemoryBackend {
    /// Create a new in-memory backend
    pub fn new() -> Self {
        Self {
            chunks: RwLock::new(HashMap::new()),
            max_capacity: 0,
            bytes_used: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
        }
    }

    /// Create with a maximum capacity
    pub fn with_capacity(max_bytes: u64) -> Self {
        Self {
            chunks: RwLock::new(HashMap::new()),
            max_capacity: max_bytes,
            bytes_used: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
        }
    }

    /// Clear all stored chunks
    pub fn clear(&self) {
        let mut chunks = self.chunks.write();
        chunks.clear();
        self.bytes_used.store(0, Ordering::SeqCst);
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageBackendSync for MemoryBackend {
    fn put(&self, id: ChunkId, data: Bytes) -> Result<()> {
        let data_len = data.len() as u64;

        // Check capacity
        if self.max_capacity > 0 {
            let current = self.bytes_used.load(Ordering::SeqCst);
            if current + data_len > self.max_capacity {
                return Err(cyxcloud_core::error::CyxCloudError::StorageFull {
                    used: current,
                    capacity: self.max_capacity,
                });
            }
        }

        let mut chunks = self.chunks.write();

        // If replacing, subtract old size
        if let Some(old) = chunks.get(&id) {
            self.bytes_used
                .fetch_sub(old.len() as u64, Ordering::SeqCst);
        }

        chunks.insert(id, data);
        self.bytes_used.fetch_add(data_len, Ordering::SeqCst);
        self.writes.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    fn get(&self, id: ChunkId) -> Result<Option<Bytes>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let chunks = self.chunks.read();
        Ok(chunks.get(&id).cloned())
    }

    fn delete(&self, id: ChunkId) -> Result<bool> {
        let mut chunks = self.chunks.write();

        if let Some(old) = chunks.remove(&id) {
            self.bytes_used
                .fetch_sub(old.len() as u64, Ordering::SeqCst);
            self.deletes.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn exists(&self, id: ChunkId) -> Result<bool> {
        let chunks = self.chunks.read();
        Ok(chunks.contains_key(&id))
    }

    fn stats(&self) -> Result<StorageStats> {
        let chunks = self.chunks.read();
        Ok(StorageStats {
            chunk_count: chunks.len() as u64,
            bytes_used: self.bytes_used.load(Ordering::SeqCst),
            bytes_capacity: self.max_capacity,
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            deletes: self.deletes.load(Ordering::Relaxed),
            avg_read_latency_us: 0, // Negligible for memory
            avg_write_latency_us: 0,
        })
    }

    fn list_chunks(&self) -> Result<Vec<ChunkId>> {
        let chunks = self.chunks.read();
        Ok(chunks.keys().copied().collect())
    }

    fn flush(&self) -> Result<()> {
        // No-op for in-memory storage
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_get() {
        let backend = MemoryBackend::new();
        let id = ChunkId::from_data(b"test");
        let data = Bytes::from_static(b"hello world");

        backend.put(id, data.clone()).unwrap();
        let retrieved = backend.get(id).unwrap().unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_delete() {
        let backend = MemoryBackend::new();
        let id = ChunkId::from_data(b"test");
        let data = Bytes::from_static(b"hello");

        backend.put(id, data).unwrap();
        assert!(backend.exists(id).unwrap());

        assert!(backend.delete(id).unwrap());
        assert!(!backend.exists(id).unwrap());

        // Deleting non-existent returns false
        assert!(!backend.delete(id).unwrap());
    }

    #[test]
    fn test_capacity_limit() {
        let backend = MemoryBackend::with_capacity(100);
        let id1 = ChunkId::from_data(b"1");
        let id2 = ChunkId::from_data(b"2");

        // This should succeed (50 bytes)
        backend.put(id1, Bytes::from(vec![0u8; 50])).unwrap();

        // This should succeed (50 more = 100 total)
        backend.put(id2, Bytes::from(vec![0u8; 50])).unwrap();

        // This should fail (would exceed 100)
        let id3 = ChunkId::from_data(b"3");
        let result = backend.put(id3, Bytes::from(vec![0u8; 1]));
        assert!(result.is_err());
    }

    #[test]
    fn test_stats() {
        let backend = MemoryBackend::with_capacity(1000);
        let id = ChunkId::from_data(b"test");

        backend.put(id, Bytes::from(vec![0u8; 100])).unwrap();
        backend.get(id).unwrap();
        backend.get(id).unwrap();

        let stats = backend.stats().unwrap();
        assert_eq!(stats.chunk_count, 1);
        assert_eq!(stats.bytes_used, 100);
        assert_eq!(stats.writes, 1);
        assert_eq!(stats.reads, 2);
        assert_eq!(stats.usage_percent(), 10.0);
    }

    #[test]
    fn test_list_chunks() {
        let backend = MemoryBackend::new();

        let ids: Vec<ChunkId> = (0..5).map(|i| ChunkId::from_data(&[i])).collect();

        for id in &ids {
            backend.put(*id, Bytes::from_static(b"data")).unwrap();
        }

        let listed = backend.list_chunks().unwrap();
        assert_eq!(listed.len(), 5);
        for id in &ids {
            assert!(listed.contains(id));
        }
    }
}
