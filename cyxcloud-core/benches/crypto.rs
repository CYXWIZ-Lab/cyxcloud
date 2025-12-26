//! Benchmarks for cryptographic operations
//!
//! Run with: cargo bench --package cyxcloud-core --bench crypto

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use cyxcloud_core::crypto::{
    encrypt, decrypt, encrypt_to_bytes, decrypt_from_bytes,
    ContentHash, EncryptionKey,
};

/// Generate test data of specified size
fn generate_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Benchmark Blake3 hashing at various sizes
fn bench_blake3_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake3_hash");

    for size in [
        1024,                  // 1 KB
        64 * 1024,             // 64 KB
        1024 * 1024,           // 1 MB
        10 * 1024 * 1024,      // 10 MB
        100 * 1024 * 1024,     // 100 MB
    ] {
        let data = generate_data(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("sequential", format_size(size)),
            &data,
            |b, data| {
                b.iter(|| ContentHash::compute(black_box(data)))
            },
        );
    }

    group.finish();
}

/// Benchmark parallel Blake3 hashing (for large data)
fn bench_blake3_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake3_parallel");

    for size in [
        10 * 1024 * 1024,      // 10 MB
        50 * 1024 * 1024,      // 50 MB
        100 * 1024 * 1024,     // 100 MB
    ] {
        let data = generate_data(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("parallel", format_size(size)),
            &data,
            |b, data| {
                b.iter(|| ContentHash::compute_parallel(black_box(data)))
            },
        );
    }

    group.finish();
}

/// Compare sequential vs parallel hashing
fn bench_hash_seq_vs_parallel(c: &mut Criterion) {
    let data = generate_data(50 * 1024 * 1024); // 50 MB

    let mut group = c.benchmark_group("hash_seq_vs_parallel_50MB");
    group.throughput(Throughput::Bytes(data.len() as u64));

    group.bench_function("sequential", |b| {
        b.iter(|| ContentHash::compute(black_box(&data)))
    });

    group.bench_function("parallel", |b| {
        b.iter(|| ContentHash::compute_parallel(black_box(&data)))
    });

    group.finish();
}

/// Benchmark AES-256-GCM encryption
fn bench_aes_encrypt(c: &mut Criterion) {
    let key = EncryptionKey::generate();

    let mut group = c.benchmark_group("aes_gcm_encrypt");

    for size in [
        1024,                  // 1 KB
        64 * 1024,             // 64 KB
        1024 * 1024,           // 1 MB
        4 * 1024 * 1024,       // 4 MB (typical chunk size)
    ] {
        let data = generate_data(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("encrypt", format_size(size)),
            &data,
            |b, data| {
                b.iter(|| encrypt(black_box(data), &key))
            },
        );
    }

    group.finish();
}

/// Benchmark AES-256-GCM decryption
fn bench_aes_decrypt(c: &mut Criterion) {
    let key = EncryptionKey::generate();

    let mut group = c.benchmark_group("aes_gcm_decrypt");

    for size in [
        1024,                  // 1 KB
        64 * 1024,             // 64 KB
        1024 * 1024,           // 1 MB
        4 * 1024 * 1024,       // 4 MB
    ] {
        let data = generate_data(size);
        let encrypted = encrypt(&data, &key).unwrap();

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("decrypt", format_size(size)),
            &encrypted,
            |b, encrypted| {
                b.iter(|| decrypt(black_box(encrypted), &key))
            },
        );
    }

    group.finish();
}

/// Benchmark encrypt + decrypt roundtrip
fn bench_encrypt_roundtrip(c: &mut Criterion) {
    let key = EncryptionKey::generate();
    let data = generate_data(4 * 1024 * 1024); // 4 MB chunk

    let mut group = c.benchmark_group("encrypt_roundtrip_4MB");
    group.throughput(Throughput::Bytes(data.len() as u64));

    group.bench_function("to_struct", |b| {
        b.iter(|| {
            let encrypted = encrypt(black_box(&data), &key).unwrap();
            decrypt(&encrypted, &key).unwrap()
        })
    });

    group.bench_function("to_bytes", |b| {
        b.iter(|| {
            let encrypted = encrypt_to_bytes(black_box(&data), &key).unwrap();
            decrypt_from_bytes(&encrypted, &key).unwrap()
        })
    });

    group.finish();
}

/// Benchmark key generation
fn bench_key_generation(c: &mut Criterion) {
    c.bench_function("key_generate", |b| {
        b.iter(|| EncryptionKey::generate())
    });
}

/// Benchmark hash verification
fn bench_hash_verify(c: &mut Criterion) {
    let data = generate_data(1024 * 1024); // 1 MB
    let hash = ContentHash::compute(&data);

    c.bench_function("hash_verify_1MB", |b| {
        b.iter(|| hash.verify(black_box(&data)))
    });
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
    bench_blake3_hash,
    bench_blake3_parallel,
    bench_hash_seq_vs_parallel,
    bench_aes_encrypt,
    bench_aes_decrypt,
    bench_encrypt_roundtrip,
    bench_key_generation,
    bench_hash_verify,
);
criterion_main!(benches);
