//! Benchmarks for Reed-Solomon erasure coding
//!
//! Run with: cargo bench --package cyxcloud-core

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use cyxcloud_core::erasure::{ErasureConfig, ErasureEncoder};

/// Generate test data of specified size
fn generate_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Benchmark encoding at various data sizes
fn bench_encode(c: &mut Criterion) {
    let encoder = ErasureEncoder::new().unwrap();

    let mut group = c.benchmark_group("erasure_encode");

    for size in [
        1024 * 1024,           // 1 MB
        4 * 1024 * 1024,       // 4 MB
        10 * 1024 * 1024,      // 10 MB
        64 * 1024 * 1024,      // 64 MB
    ] {
        let data = generate_data(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("sequential", format!("{}MB", size / (1024 * 1024))),
            &data,
            |b, data| {
                b.iter(|| encoder.encode(black_box(data)))
            },
        );
    }

    group.finish();
}

/// Benchmark parallel encoding at various data sizes
fn bench_encode_parallel(c: &mut Criterion) {
    let encoder = ErasureEncoder::new().unwrap();

    let mut group = c.benchmark_group("erasure_encode_parallel");

    for size in [
        4 * 1024 * 1024,       // 4 MB
        10 * 1024 * 1024,      // 10 MB
        64 * 1024 * 1024,      // 64 MB
        100 * 1024 * 1024,     // 100 MB
    ] {
        let data = generate_data(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("parallel", format!("{}MB", size / (1024 * 1024))),
            &data,
            |b, data| {
                b.iter(|| encoder.encode_parallel(black_box(data)))
            },
        );
    }

    group.finish();
}

/// Benchmark decoding with various numbers of missing shards
fn bench_decode(c: &mut Criterion) {
    let encoder = ErasureEncoder::new().unwrap();
    let data = generate_data(10 * 1024 * 1024); // 10 MB
    let original_size = data.len();

    // Encode once
    let shards = encoder.encode(&data).unwrap();

    let mut group = c.benchmark_group("erasure_decode");
    group.throughput(Throughput::Bytes(original_size as u64));

    // Decode with 0 missing shards
    {
        let shard_opts: Vec<_> = shards.iter().cloned().map(Some).collect();
        group.bench_function("0_missing", |b| {
            b.iter(|| encoder.decode(black_box(&shard_opts), original_size))
        });
    }

    // Decode with 2 missing shards
    {
        let mut shard_opts: Vec<_> = shards.iter().cloned().map(Some).collect();
        shard_opts[0] = None;
        shard_opts[7] = None;
        group.bench_function("2_missing", |b| {
            b.iter(|| encoder.decode(black_box(&shard_opts), original_size))
        });
    }

    // Decode with 4 missing shards (maximum)
    {
        let mut shard_opts: Vec<_> = shards.iter().cloned().map(Some).collect();
        shard_opts[0] = None;
        shard_opts[3] = None;
        shard_opts[10] = None;
        shard_opts[13] = None;
        group.bench_function("4_missing", |b| {
            b.iter(|| encoder.decode(black_box(&shard_opts), original_size))
        });
    }

    group.finish();
}

/// Benchmark shard verification
fn bench_verify(c: &mut Criterion) {
    let encoder = ErasureEncoder::new().unwrap();
    let data = generate_data(10 * 1024 * 1024); // 10 MB
    let shards = encoder.encode(&data).unwrap();

    c.bench_function("verify_shards_10MB", |b| {
        b.iter(|| encoder.verify_shards(black_box(&shards)))
    });
}

/// Compare sequential vs parallel encoding
fn bench_seq_vs_parallel(c: &mut Criterion) {
    let encoder = ErasureEncoder::new().unwrap();
    let data = generate_data(50 * 1024 * 1024); // 50 MB

    let mut group = c.benchmark_group("seq_vs_parallel_50MB");
    group.throughput(Throughput::Bytes(data.len() as u64));

    group.bench_function("sequential", |b| {
        b.iter(|| encoder.encode(black_box(&data)))
    });

    group.bench_function("parallel", |b| {
        b.iter(|| encoder.encode_parallel(black_box(&data)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_encode,
    bench_encode_parallel,
    bench_decode,
    bench_verify,
    bench_seq_vs_parallel,
);
criterion_main!(benches);
