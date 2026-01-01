//! Benchmarks for storage backends
//!
//! Run with: cargo bench --package cyxcloud-storage --bench storage

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use cyxcloud_core::chunk::ChunkId;
use cyxcloud_storage::backend::StorageBackendSync;
use cyxcloud_storage::memory::MemoryBackend;
use cyxcloud_storage::rocks::RocksDbBackend;
use cyxcloud_storage::StorageConfig;
use tempfile::TempDir;

/// Generate test data of specified size
fn generate_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Generate a unique chunk ID
fn generate_chunk_id(seed: u64) -> ChunkId {
    ChunkId::from_data(&seed.to_le_bytes())
}

/// Benchmark single put operations (latency)
fn bench_put_latency(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    let mut group = c.benchmark_group("rocksdb_put_latency");

    for size in [
        1024,            // 1 KB
        64 * 1024,       // 64 KB
        256 * 1024,      // 256 KB (typical shard)
        1024 * 1024,     // 1 MB
        4 * 1024 * 1024, // 4 MB (typical chunk)
    ] {
        let data = Bytes::from(generate_data(size));

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("put", format_size(size)),
            &data,
            |b, data| {
                let mut counter = 0u64;
                b.iter(|| {
                    counter += 1;
                    let id = generate_chunk_id(counter);
                    backend.put(id, black_box(data.clone())).unwrap()
                })
            },
        );
    }

    group.finish();
}

/// Benchmark get operations (latency)
fn bench_get_latency(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    let mut group = c.benchmark_group("rocksdb_get_latency");

    for size in [
        1024,            // 1 KB
        64 * 1024,       // 64 KB
        256 * 1024,      // 256 KB
        1024 * 1024,     // 1 MB
        4 * 1024 * 1024, // 4 MB
    ] {
        // Pre-populate with data
        let data = Bytes::from(generate_data(size));
        let ids: Vec<ChunkId> = (0..100).map(|i| generate_chunk_id(i)).collect();
        for id in &ids {
            backend.put(*id, data.clone()).unwrap();
        }
        backend.flush().unwrap();

        let mut idx = 0usize;
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("get", format_size(size)),
            &ids,
            |b, ids| {
                b.iter(|| {
                    idx = (idx + 1) % ids.len();
                    backend.get(black_box(ids[idx])).unwrap()
                })
            },
        );
    }

    group.finish();
}

/// Benchmark sequential write throughput
fn bench_write_throughput(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    // 4 MB chunks, write 64 MB total per iteration
    let chunk_size = 4 * 1024 * 1024;
    let chunks_per_iter = 16;
    let total_bytes = chunk_size * chunks_per_iter;

    let data = Bytes::from(generate_data(chunk_size));

    let mut group = c.benchmark_group("rocksdb_write_throughput");
    group.throughput(Throughput::Bytes(total_bytes as u64));
    group.sample_size(20); // Fewer samples for expensive test

    let mut counter = 0u64;
    group.bench_function("sequential_64MB", |b| {
        b.iter(|| {
            for _ in 0..chunks_per_iter {
                counter += 1;
                let id = generate_chunk_id(counter);
                backend.put(id, data.clone()).unwrap();
            }
        })
    });

    group.finish();
}

/// Benchmark sequential read throughput
fn bench_read_throughput(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    // Pre-populate with 4 MB chunks
    let chunk_size = 4 * 1024 * 1024;
    let chunks_per_iter = 16;
    let total_bytes = chunk_size * chunks_per_iter;

    let data = Bytes::from(generate_data(chunk_size));
    let ids: Vec<ChunkId> = (0..chunks_per_iter as u64)
        .map(|i| generate_chunk_id(i))
        .collect();

    for id in &ids {
        backend.put(*id, data.clone()).unwrap();
    }
    backend.flush().unwrap();

    let mut group = c.benchmark_group("rocksdb_read_throughput");
    group.throughput(Throughput::Bytes(total_bytes as u64));
    group.sample_size(50);

    group.bench_function("sequential_64MB", |b| {
        b.iter(|| {
            for id in &ids {
                black_box(backend.get(*id).unwrap());
            }
        })
    });

    group.finish();
}

/// Benchmark mixed read/write workload
fn bench_mixed_workload(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    let chunk_size = 256 * 1024; // 256 KB shards
    let data = Bytes::from(generate_data(chunk_size));

    // Pre-populate with some data
    let pre_ids: Vec<ChunkId> = (0..100).map(|i| generate_chunk_id(i)).collect();
    for id in &pre_ids {
        backend.put(*id, data.clone()).unwrap();
    }
    backend.flush().unwrap();

    let mut group = c.benchmark_group("rocksdb_mixed_workload");
    // Approximate: 5 writes + 5 reads = 10 ops * 256KB
    group.throughput(Throughput::Bytes(10 * chunk_size as u64));

    let mut write_counter = 1000u64;
    let mut read_idx = 0usize;

    group.bench_function("50_50_read_write", |b| {
        b.iter(|| {
            // 5 writes
            for _ in 0..5 {
                write_counter += 1;
                let id = generate_chunk_id(write_counter);
                backend.put(id, data.clone()).unwrap();
            }
            // 5 reads
            for _ in 0..5 {
                read_idx = (read_idx + 1) % pre_ids.len();
                black_box(backend.get(pre_ids[read_idx]).unwrap());
            }
        })
    });

    group.finish();
}

/// Benchmark memory backend for comparison
fn bench_memory_backend(c: &mut Criterion) {
    let backend = MemoryBackend::new();
    let chunk_size = 4 * 1024 * 1024; // 4 MB
    let data = Bytes::from(generate_data(chunk_size));

    let mut group = c.benchmark_group("memory_backend");
    group.throughput(Throughput::Bytes(chunk_size as u64));

    let mut counter = 0u64;
    group.bench_function("put_4MB", |b| {
        b.iter(|| {
            counter += 1;
            let id = generate_chunk_id(counter);
            backend.put(id, black_box(data.clone())).unwrap()
        })
    });

    // Pre-populate for read test
    let ids: Vec<ChunkId> = (10000..10100).map(|i| generate_chunk_id(i)).collect();
    for id in &ids {
        backend.put(*id, data.clone()).unwrap();
    }

    let mut idx = 0usize;
    group.bench_function("get_4MB", |b| {
        b.iter(|| {
            idx = (idx + 1) % ids.len();
            black_box(backend.get(ids[idx]).unwrap())
        })
    });

    group.finish();
}

/// Compare RocksDB vs Memory backend
fn bench_rocksdb_vs_memory(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let rocks_backend = RocksDbBackend::open(config).unwrap();
    let memory_backend = MemoryBackend::new();

    let chunk_size = 256 * 1024; // 256 KB
    let data = Bytes::from(generate_data(chunk_size));

    let mut group = c.benchmark_group("rocksdb_vs_memory_256KB");
    group.throughput(Throughput::Bytes(chunk_size as u64));

    let mut counter = 0u64;
    group.bench_function("rocksdb_put", |b| {
        b.iter(|| {
            counter += 1;
            let id = generate_chunk_id(counter);
            rocks_backend.put(id, data.clone()).unwrap()
        })
    });

    let mut counter2 = 0u64;
    group.bench_function("memory_put", |b| {
        b.iter(|| {
            counter2 += 1;
            let id = generate_chunk_id(counter2 + 1_000_000);
            memory_backend.put(id, data.clone()).unwrap()
        })
    });

    group.finish();
}

/// Benchmark exists check performance
fn bench_exists(c: &mut Criterion) {
    let temp_dir = TempDir::new().unwrap();
    let config = StorageConfig::new(temp_dir.path());
    let backend = RocksDbBackend::open(config).unwrap();

    // Pre-populate
    let data = Bytes::from_static(b"test data");
    let existing_ids: Vec<ChunkId> = (0..100).map(|i| generate_chunk_id(i)).collect();
    let missing_ids: Vec<ChunkId> = (1000..1100).map(|i| generate_chunk_id(i)).collect();

    for id in &existing_ids {
        backend.put(*id, data.clone()).unwrap();
    }
    backend.flush().unwrap();

    let mut group = c.benchmark_group("rocksdb_exists");

    let mut idx = 0usize;
    group.bench_function("existing_key", |b| {
        b.iter(|| {
            idx = (idx + 1) % existing_ids.len();
            black_box(backend.exists(existing_ids[idx]).unwrap())
        })
    });

    let mut idx2 = 0usize;
    group.bench_function("missing_key", |b| {
        b.iter(|| {
            idx2 = (idx2 + 1) % missing_ids.len();
            black_box(backend.exists(missing_ids[idx2]).unwrap())
        })
    });

    group.finish();
}

/// Format size for display
fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{}MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{}B", bytes)
    }
}

criterion_group!(
    benches,
    bench_put_latency,
    bench_get_latency,
    bench_write_throughput,
    bench_read_throughput,
    bench_mixed_workload,
    bench_memory_backend,
    bench_rocksdb_vs_memory,
    bench_exists,
);
criterion_main!(benches);
