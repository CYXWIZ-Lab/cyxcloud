//! RocksDB storage backend
//!
//! Production-grade chunk storage using RocksDB LSM tree.
//! Optimized for large values (chunks) with high write throughput.

use crate::backend::{StorageBackendSync, StorageStats};
use crate::StorageConfig;
use bytes::Bytes;
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_core::error::{CyxCloudError, Result};
use parking_lot::Mutex;
use rocksdb::{
    BlockBasedOptions, Cache, DBCompressionType, Options, ReadOptions, WriteBatch, WriteOptions, DB,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::{debug, info};

/// Column family names
const CF_CHUNKS: &str = "chunks";
const CF_METADATA: &str = "metadata";

/// Metadata entry `SIZE_PREFIX ++ chunk id` holds the chunk's byte length
/// (u64 LE), written in the same batch as the chunk, so the totals come from
/// these small entries and a put never reads the chunk itself.
const SIZE_PREFIX: &[u8] = b"size:";
/// Present once every stored chunk has its size entry (stores written before
/// size entries get them in one scan at the next open).
const SIZES_INDEXED: &[u8] = b"sizes-indexed";
/// Locks that make a put's or delete's check and write one step per chunk id.
const STRIPES: usize = 64;

fn size_key(id: &[u8]) -> Vec<u8> {
    [SIZE_PREFIX, id].concat()
}

fn read_u64(bytes: &[u8]) -> Option<u64> {
    bytes.try_into().ok().map(u64::from_le_bytes)
}

fn add_saturating(counter: &AtomicU64, v: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| {
        Some(x.saturating_add(v))
    });
}

fn sub_saturating(counter: &AtomicU64, v: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| {
        Some(x.saturating_sub(v))
    });
}

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

    /// Chunks stored and their bytes (value lengths): summed from the size
    /// entries at open, then kept by put and delete, so stats() is cheap
    /// (the health check and the heartbeat call it every few seconds).
    chunk_count: AtomicU64,
    bytes_used: AtomicU64,

    /// One lock per stripe of chunk ids (see STRIPES)
    stripes: Vec<Mutex<()>>,
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
        // Answers "not stored" for a new chunk without reading the levels.
        block_opts.set_bloom_filter(10.0, false);
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

        if db
            .get_cf(&Self::cf(&db, CF_METADATA)?, SIZES_INDEXED)
            .map_err(|e| CyxCloudError::Storage(format!("Read failed: {}", e)))?
            .is_none()
        {
            Self::index_sizes(&db)?;
        }
        let (chunk_count, bytes_used) = Self::sum_sizes(&db)?;
        info!(
            chunk_count,
            bytes_used, "RocksDB storage opened successfully"
        );

        Ok(Self {
            db,
            config,
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            read_latency_total_us: AtomicU64::new(0),
            write_latency_total_us: AtomicU64::new(0),
            chunk_count: AtomicU64::new(chunk_count),
            bytes_used: AtomicU64::new(bytes_used),
            stripes: (0..STRIPES).map(|_| Mutex::new(())).collect(),
        })
    }

    fn cf<'a>(db: &'a DB, name: &str) -> Result<std::sync::Arc<rocksdb::BoundColumnFamily<'a>>> {
        db.cf_handle(name)
            .ok_or_else(|| CyxCloudError::Storage(format!("{} column family missing", name)))
    }

    /// Give every stored chunk its size entry (a store written before size
    /// entries): one scan of the chunks, outside the block cache.
    fn index_sizes(db: &DB) -> Result<()> {
        info!("Indexing stored chunk sizes (once per store)");
        let chunks = Self::cf(db, CF_CHUNKS)?;
        let meta = Self::cf(db, CF_METADATA)?;
        let mut read_opts = ReadOptions::default();
        read_opts.fill_cache(false);
        let mut batch = WriteBatch::default();
        for item in db.iterator_cf_opt(&chunks, read_opts, rocksdb::IteratorMode::Start) {
            let (key, value) =
                item.map_err(|e| CyxCloudError::Storage(format!("Scan failed: {}", e)))?;
            batch.put_cf(&meta, size_key(&key), (value.len() as u64).to_le_bytes());
            if batch.len() >= 1000 {
                db.write(std::mem::take(&mut batch))
                    .map_err(|e| CyxCloudError::Storage(format!("Write failed: {}", e)))?;
            }
        }
        batch.put_cf(&meta, SIZES_INDEXED, []);
        db.write(batch)
            .map_err(|e| CyxCloudError::Storage(format!("Write failed: {}", e)))
    }

    /// Chunk count and bytes from the size entries.
    fn sum_sizes(db: &DB) -> Result<(u64, u64)> {
        let meta = Self::cf(db, CF_METADATA)?;
        let (mut count, mut bytes) = (0u64, 0u64);
        let mode = rocksdb::IteratorMode::From(SIZE_PREFIX, rocksdb::Direction::Forward);
        for item in db.iterator_cf(&meta, mode) {
            let (key, value) =
                item.map_err(|e| CyxCloudError::Storage(format!("Scan failed: {}", e)))?;
            if !key.starts_with(SIZE_PREFIX) {
                break;
            }
            count += 1;
            bytes = bytes.saturating_add(read_u64(&value).unwrap_or(0));
        }
        Ok((count, bytes))
    }

    /// The stored size of a chunk, from its size entry (None: not stored).
    fn stored_size(&self, key: &[u8]) -> Result<Option<u64>> {
        Ok(self
            .db
            .get_pinned_cf(&self.cf_metadata(), size_key(key))
            .map_err(|e| CyxCloudError::Storage(format!("Read failed: {}", e)))?
            .and_then(|v| read_u64(&v)))
    }

    fn stripe(&self, key: &[u8]) -> &Mutex<()> {
        &self.stripes[key.first().copied().unwrap_or(0) as usize % STRIPES]
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
}

impl StorageBackendSync for RocksDbBackend {
    fn put(&self, id: ChunkId, data: Bytes) -> Result<()> {
        let start = Instant::now();
        let key = id.as_bytes();

        // Check, write and count as one step for this id.
        let _guard = self.stripe(key).lock();

        // Chunk ids are content hashes: an id already stored holds the same
        // bytes (or a damaged copy being repaired), so writing it again
        // needs no new room and does not count again.
        let existing = self.stored_size(key)?;
        let len = data.len() as u64;

        // Check capacity if set
        if existing.is_none() && self.config.max_capacity > 0 {
            let current_size = self.bytes_used.load(Ordering::Relaxed);
            if current_size.saturating_add(len) > self.config.max_capacity {
                return Err(CyxCloudError::StorageFull {
                    used: current_size,
                    capacity: self.config.max_capacity,
                });
            }
        }

        // Configure write options
        let mut write_opts = WriteOptions::default();
        write_opts.set_sync(false); // Async writes for performance

        // The chunk and its size entry in one batch
        let mut batch = WriteBatch::default();
        batch.put_cf(&self.cf_chunks(), key, &data);
        batch.put_cf(&self.cf_metadata(), size_key(key), len.to_le_bytes());
        self.db
            .write_opt(batch, &write_opts)
            .map_err(|e| CyxCloudError::Storage(format!("Write failed: {}", e)))?;
        match existing {
            None => {
                add_saturating(&self.chunk_count, 1);
                add_saturating(&self.bytes_used, len);
            }
            Some(old) => {
                add_saturating(&self.bytes_used, len);
                sub_saturating(&self.bytes_used, old);
            }
        }

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

        let _guard = self.stripe(key).lock();

        // Its size first (and whether it is there at all)
        let Some(size) = self.stored_size(key)? else {
            return Ok(false);
        };

        let mut batch = WriteBatch::default();
        batch.delete_cf(&self.cf_chunks(), key);
        batch.delete_cf(&self.cf_metadata(), size_key(key));
        self.db
            .write(batch)
            .map_err(|e| CyxCloudError::Storage(format!("Delete failed: {}", e)))?;
        sub_saturating(&self.chunk_count, 1);
        sub_saturating(&self.bytes_used, size);

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
        let chunk_count = self.chunk_count.load(Ordering::Relaxed);
        let bytes_used = self.bytes_used.load(Ordering::Relaxed);

        // Calculate average latencies
        let reads = self.reads.load(Ordering::Relaxed);
        let writes = self.writes.load(Ordering::Relaxed);
        let read_latency_total = self.read_latency_total_us.load(Ordering::Relaxed);
        let write_latency_total = self.write_latency_total_us.load(Ordering::Relaxed);

        let avg_read_latency_us = read_latency_total.checked_div(reads).unwrap_or(0);
        let avg_write_latency_us = write_latency_total.checked_div(writes).unwrap_or(0);

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
    fn stats_follow_puts_and_deletes_and_survive_a_reopen() {
        let temp_dir = TempDir::new().unwrap();
        let backend = RocksDbBackend::open(StorageConfig::new(temp_dir.path())).unwrap();
        let a = ChunkId::from_data(b"a");
        let b = ChunkId::from_data(b"b");
        backend.put(a, Bytes::from_static(b"12345")).unwrap();
        backend.put(a, Bytes::from_static(b"12345")).unwrap(); // same chunk again
        backend.put(b, Bytes::from_static(b"123")).unwrap();
        let s = backend.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (2, 8));
        assert!(backend.delete(a).unwrap());
        assert!(!backend.delete(a).unwrap());
        let s = backend.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (1, 3));
        drop(backend);
        let reopened = RocksDbBackend::open(StorageConfig::new(temp_dir.path())).unwrap();
        let s = reopened.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (1, 3));
    }

    #[test]
    fn concurrent_puts_and_deletes_of_one_chunk_count_it_once() {
        let temp_dir = TempDir::new().unwrap();
        let backend =
            std::sync::Arc::new(RocksDbBackend::open(StorageConfig::new(temp_dir.path())).unwrap());
        let id = ChunkId::from_data(b"shared");
        let run = |f: fn(&RocksDbBackend, ChunkId)| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let b = backend.clone();
                    std::thread::spawn(move || f(&b, id))
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
        };
        run(|b, id| b.put(id, Bytes::from_static(b"1234")).unwrap());
        let s = backend.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (1, 4));
        run(|b, id| {
            b.delete(id).unwrap();
        });
        let s = backend.stats().unwrap();
        assert_eq!(
            (s.chunk_count, s.bytes_used),
            (0, 0),
            "no double count, no wrap"
        );
    }

    #[test]
    fn a_store_without_size_entries_is_indexed_once_at_open() {
        let temp_dir = TempDir::new().unwrap();
        {
            let backend = RocksDbBackend::open(StorageConfig::new(temp_dir.path())).unwrap();
            backend
                .put(ChunkId::from_data(b"x"), Bytes::from_static(b"abcdef"))
                .unwrap();
            backend
                .put(ChunkId::from_data(b"y"), Bytes::from_static(b"gh"))
                .unwrap();
            // As a store written before size entries: chunks only.
            let meta = backend.cf_metadata();
            for id in [ChunkId::from_data(b"x"), ChunkId::from_data(b"y")] {
                backend
                    .db
                    .delete_cf(&meta, size_key(id.as_bytes()))
                    .unwrap();
            }
            backend.db.delete_cf(&meta, SIZES_INDEXED).unwrap();
        }
        let reopened = RocksDbBackend::open(StorageConfig::new(temp_dir.path())).unwrap();
        let s = reopened.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (2, 8));
    }

    #[test]
    fn repairing_a_chunk_with_other_bytes_keeps_one_count() {
        let (backend, _dir) = create_test_backend();
        let id = ChunkId::from_data(b"r");
        backend.put(id, Bytes::from_static(b"damaged!!")).unwrap();
        backend.put(id, Bytes::from_static(b"good")).unwrap();
        let s = backend.stats().unwrap();
        assert_eq!((s.chunk_count, s.bytes_used), (1, 4));
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
