//! RocksDB storage backend
//!
//! Production-grade chunk storage using RocksDB LSM tree.
//! Optimized for large values (chunks) with high write throughput.

use crate::backend::{StorageBackendSync, StorageStats};
use crate::StorageConfig;
use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::{CyxCloudError, Result};
use parking_lot::RwLock;
use rocksdb::{BlockBasedOptions, Cache, DBCompressionType, Options, WriteOptions, DB};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, info};

/// Column family names
const CF_CHUNKS: &str = "chunks";
const CF_METADATA: &str = "metadata";

/// RocksDB-based storage backend
pub struct RocksDbBackend {
    /// RocksDB instance
    db: DB,

    /// Configuration
    config: StorageConfig,

    /// Operation counters
    reads: AtomicU64,
    writes: AtomicU64,
    deletes: AtomicU64,

    /// Latency tracking (cumulative microseconds)
    read_latency_total_us: AtomicU64,
    write_latency_total_us: AtomicU64,

    /// Cached statistics (updated periodically)
    cached_stats: RwLock<StorageStats>,
}

impl RocksDbBackend {
    /// Open or create a RocksDB storage at the given path
    pub fn open(config: StorageConfig) -> Result<Self> {
        info!(path = ?config.path, "Opening RocksDB storage");

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        // Performance tuning for chunk storage
        opts.set_max_open_files(1000);
        opts.set_keep_log_file_num(10);
        opts.set_max_total_wal_size(256 * 1024 * 1024); // 256 MB WAL
        opts.increase_parallelism(config.compaction_threads as i32);
        opts.set_max_background_jobs(config.compaction_threads as i32);

        // Compression for chunks (LZ4 is fast)
        if config.compression {
            opts.set_compression_type(DBCompressionType::Lz4);
        }

        // Block cache for read performance
        let cache = Cache::new_lru_cache(config.cache_size);
        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_block_cache(&cache);
        block_opts.set_block_size(64 * 1024); // 64KB blocks (good for 4MB chunks)
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
        opts.set_block_based_table_factory(&block_opts);

        // Optimize for large values (chunks are typically 256KB - 64MB)
        opts.set_min_write_buffer_number(2);
        opts.set_max_write_buffer_number(4);
        opts.set_write_buffer_size(64 * 1024 * 1024); // 64 MB write buffer

        // Column family options
        let cf_descriptors = vec![
            rocksdb::ColumnFamilyDescriptor::new(CF_CHUNKS, opts.clone()),
            rocksdb::ColumnFamilyDescriptor::new(CF_METADATA, Options::default()),
        ];

        // Create directory if it doesn't exist
        std::fs::create_dir_all(&config.path).map_err(|e| {
            CyxCloudError::Storage(format!("Failed to create storage directory: {}", e))
        })?;

        // Open database
        let db = DB::open_cf_descriptors(&opts, &config.path, cf_descriptors)
            .map_err(|e| CyxCloudError::Storage(format!("Failed to open RocksDB: {}", e)))?;

        info!("RocksDB storage opened successfully");

        Ok(Self {
            db,
            config,
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            read_latency_total_us: AtomicU64::new(0),
            write_latency_total_us: AtomicU64::new(0),
            cached_stats: RwLock::new(StorageStats::default()),
        })
    }

    /// Open with default configuration
    pub fn open_default<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open(StorageConfig::new(path.as_ref()))
    }

    /// Get the chunks column family handle
    fn cf_chunks(&self) -> std::sync::Arc<rocksdb::BoundColumnFamily<'_>> {
        self.db
            .cf_handle(CF_CHUNKS)
            .expect("Chunks column family should exist")
    }

    /// Get the metadata column family handle
    #[allow(dead_code)]
    fn cf_metadata(&self) -> std::sync::Arc<rocksdb::BoundColumnFamily<'_>> {
        self.db
            .cf_handle(CF_METADATA)
            .expect("Metadata column family should exist")
    }

    /// Compact the database (call periodically for performance)
    pub fn compact(&self) {
        info!("Starting database compaction");
        self.db
            .compact_range_cf(&self.cf_chunks(), None::<&[u8]>, None::<&[u8]>);
        info!("Database compaction complete");
    }

    /// Get approximate storage size
    pub fn approximate_size(&self) -> u64 {
        // Get approximate size from RocksDB properties
        self.db
            .property_int_value("rocksdb.total-sst-files-size")
            .ok()
            .flatten()
            .unwrap_or(0)
    }
}

impl StorageBackendSync for RocksDbBackend {
    fn put(&self, id: ChunkId, data: Bytes) -> Result<()> {
        let start = Instant::now();
        let key = id.as_bytes();

        // Check capacity if set
        if self.config.max_capacity > 0 {
            let current_size = self.approximate_size();
            if current_size + data.len() as u64 > self.config.max_capacity {
                return Err(CyxCloudError::StorageFull {
                    used: current_size,
                    capacity: self.config.max_capacity,
                });
            }
        }

        // Configure write options
        let mut write_opts = WriteOptions::default();
        write_opts.set_sync(false); // Async writes for performance

        self.db
            .put_cf_opt(&self.cf_chunks(), key, &data, &write_opts)
            .map_err(|e| CyxCloudError::Storage(format!("Write failed: {}", e)))?;

        // Track latency and count
        let elapsed_us = start.elapsed().as_micros() as u64;
        self.write_latency_total_us
            .fetch_add(elapsed_us, Ordering::Relaxed);
        self.writes.fetch_add(1, Ordering::Relaxed);
        debug!(chunk_id = %id, size = data.len(), latency_us = elapsed_us, "Stored chunk");

        Ok(())
    }

    fn get(&self, id: ChunkId) -> Result<Option<Bytes>> {
        let start = Instant::now();
        let key = id.as_bytes();

        let result = self
            .db
            .get_cf(&self.cf_chunks(), key)
            .map_err(|e| CyxCloudError::Storage(format!("Read failed: {}", e)))?;

        // Track latency and count
        let elapsed_us = start.elapsed().as_micros() as u64;
        self.read_latency_total_us
            .fetch_add(elapsed_us, Ordering::Relaxed);
        self.reads.fetch_add(1, Ordering::Relaxed);

        Ok(result.map(Bytes::from))
    }

    fn delete(&self, id: ChunkId) -> Result<bool> {
        let key = id.as_bytes();

        // Check if exists first
        let exists = self.exists(id)?;
        if !exists {
            return Ok(false);
        }

        self.db
            .delete_cf(&self.cf_chunks(), key)
            .map_err(|e| CyxCloudError::Storage(format!("Delete failed: {}", e)))?;

        self.deletes.fetch_add(1, Ordering::Relaxed);
        debug!(chunk_id = %id, "Deleted chunk");

        Ok(true)
    }

    fn exists(&self, id: ChunkId) -> Result<bool> {
        let key = id.as_bytes();

        // Use key_may_exist for fast path
        if !self.db.key_may_exist_cf(&self.cf_chunks(), key) {
            return Ok(false);
        }

        // Confirm with actual read (key_may_exist can have false positives)
        let result = self
            .db
            .get_cf(&self.cf_chunks(), key)
            .map_err(|e| CyxCloudError::Storage(format!("Exists check failed: {}", e)))?;

        Ok(result.is_some())
    }

    fn stats(&self) -> Result<StorageStats> {
        // Count chunks by iterating (expensive, should be cached)
        let mut chunk_count = 0u64;
        let mut bytes_used = 0u64;

        let iter = self
            .db
            .iterator_cf(&self.cf_chunks(), rocksdb::IteratorMode::Start);

        for (_, value) in iter.flatten() {
            chunk_count += 1;
            bytes_used += value.len() as u64;
        }

        // Calculate average latencies
        let reads = self.reads.load(Ordering::Relaxed);
        let writes = self.writes.load(Ordering::Relaxed);
        let read_latency_total = self.read_latency_total_us.load(Ordering::Relaxed);
        let write_latency_total = self.write_latency_total_us.load(Ordering::Relaxed);

        let avg_read_latency_us = if reads > 0 {
            read_latency_total / reads
        } else {
            0
        };

        let avg_write_latency_us = if writes > 0 {
            write_latency_total / writes
        } else {
            0
        };

        let stats = StorageStats {
            chunk_count,
            bytes_used,
            bytes_capacity: self.config.max_capacity,
            reads,
            writes,
            deletes: self.deletes.load(Ordering::Relaxed),
            avg_read_latency_us,
            avg_write_latency_us,
        };

        // Update cached stats
        *self.cached_stats.write() = stats.clone();

        Ok(stats)
    }

    fn list_chunks(&self) -> Result<Vec<ChunkId>> {
        let mut chunks = Vec::new();

        let iter = self
            .db
            .iterator_cf(&self.cf_chunks(), rocksdb::IteratorMode::Start);

        for (key, _) in iter.flatten() {
            if key.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&key);
                chunks.push(ChunkId::from_bytes(arr));
            }
        }

        Ok(chunks)
    }

    fn flush(&self) -> Result<()> {
        self.db
            .flush()
            .map_err(|e| CyxCloudError::Storage(format!("Flush failed: {}", e)))?;

        debug!("Flushed storage to disk");
        Ok(())
    }
}

impl Drop for RocksDbBackend {
    fn drop(&mut self) {
        info!("Closing RocksDB storage");
        // RocksDB handles cleanup automatically
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_backend() -> (RocksDbBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let backend = RocksDbBackend::open(config).unwrap();
        (backend, temp_dir)
    }

    #[test]
    fn test_put_get() {
        let (backend, _dir) = create_test_backend();
        let id = ChunkId::from_data(b"test");
        let data = Bytes::from_static(b"hello world");

        backend.put(id, data.clone()).unwrap();
        let retrieved = backend.get(id).unwrap().unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::new(temp_dir.path());
        let id = ChunkId::from_data(b"persist");
        let data = Bytes::from_static(b"persistent data");

        // Write and close
        {
            let backend = RocksDbBackend::open(config.clone()).unwrap();
            backend.put(id, data.clone()).unwrap();
            backend.flush().unwrap();
        }

        // Reopen and verify
        {
            let backend = RocksDbBackend::open(config).unwrap();
            let retrieved = backend.get(id).unwrap().unwrap();
            assert_eq!(retrieved, data);
        }
    }

    #[test]
    fn test_delete() {
        let (backend, _dir) = create_test_backend();
        let id = ChunkId::from_data(b"delete_me");
        let data = Bytes::from_static(b"to be deleted");

        backend.put(id, data).unwrap();
        assert!(backend.exists(id).unwrap());

        assert!(backend.delete(id).unwrap());
        assert!(!backend.exists(id).unwrap());
    }

    #[test]
    fn test_large_chunk() {
        let (backend, _dir) = create_test_backend();
        let id = ChunkId::from_data(b"large");
        let data = Bytes::from(vec![42u8; 4 * 1024 * 1024]); // 4 MB

        backend.put(id, data.clone()).unwrap();
        let retrieved = backend.get(id).unwrap().unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_list_chunks() {
        let (backend, _dir) = create_test_backend();

        let ids: Vec<ChunkId> = (0..10).map(|i| ChunkId::from_data(&[i])).collect();

        for id in &ids {
            backend.put(*id, Bytes::from_static(b"data")).unwrap();
        }

        let listed = backend.list_chunks().unwrap();
        assert_eq!(listed.len(), 10);
    }

    #[test]
    fn test_latency_tracking() {
        let (backend, _dir) = create_test_backend();

        // Initially no operations, no latency
        let stats = backend.stats().unwrap();
        assert_eq!(stats.reads, 0);
        assert_eq!(stats.writes, 0);
        assert_eq!(stats.avg_read_latency_us, 0);
        assert_eq!(stats.avg_write_latency_us, 0);

        // Perform some writes
        for i in 0..10u8 {
            let id = ChunkId::from_data(&[i]);
            let data = Bytes::from(vec![i; 1024]);
            backend.put(id, data).unwrap();
        }

        // Check write stats
        let stats = backend.stats().unwrap();
        assert_eq!(stats.writes, 10);
        // Write latency should be non-zero (at least 1 microsecond per op typically)
        // but we don't assert on exact value since it's system-dependent

        // Perform some reads
        for i in 0..10u8 {
            let id = ChunkId::from_data(&[i]);
            backend.get(id).unwrap();
        }

        // Check read stats
        let stats = backend.stats().unwrap();
        assert_eq!(stats.reads, 10);
        assert_eq!(stats.writes, 10);
        // Average latencies should be tracked
        // They may be 0 on very fast systems, so we just check they don't panic
    }
}
