# CyxCloud: Datacenter-Grade Decentralized Storage Architecture

## 1. RAID Implementation Strategy for Decentralized Storage

### Software RAID vs Hardware RAID in Distributed Systems

**Traditional RAID (Single-Node)**:
- **RAID 5**: Striping + single parity (can lose 1 disk)
- **RAID 6**: Striping + dual parity (can lose 2 disks)
- **RAID 10**: Mirroring + striping (can lose 1 disk per mirror pair)
- **Constraint**: All disks in same physical chassis

**Distributed Erasure Coding (Multi-Node)**:
- Reed-Solomon (k=10, m=4): Can lose ANY 4 of 14 nodes
- Geographic distribution across datacenters
- Scales horizontally (add nodes, not disks)
- No single point of failure

### Hybrid Architecture (Best of Both Worlds)

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: Local RAID (Per Storage Node)                     │
│  ├─ Each server runs ZFS with RAID-Z2 (dual parity)         │
│  ├─ Protects against individual disk failures               │
│  └─ Managed by OS/filesystem, transparent to CyxCloud       │
│                                                              │
│  Layer 2: Distributed Erasure Coding (Cross-Node)           │
│  ├─ Reed-Solomon (k=10, m=4) across 14 nodes                │
│  ├─ Protects against entire node/datacenter failures        │
│  └─ Implemented in CyxCloud Rust code                       │
└─────────────────────────────────────────────────────────────┘

Storage Efficiency:
- RAID-Z2 overhead: ~20% (2 parity disks per vdev)
- Reed-Solomon overhead: 40% (4 parity shards of 14 total)
- Total effective capacity: ~48% of raw storage
- Reliability: Can survive 4 node failures + 2 disk failures per node
```

### Erasure Coding Parameters

```rust
// Optimal parameters for datacenter deployment
const DATA_SHARDS: usize = 10;      // Original data pieces
const PARITY_SHARDS: usize = 4;     // Redundancy pieces
const TOTAL_SHARDS: usize = 14;     // Total distributed nodes

// Any 10 of 14 shards can reconstruct the original data
// Can tolerate loss of 4 simultaneous nodes

// Storage overhead: 40% (4/10 = 0.4)
// Comparable to 3-way replication (200% overhead) but more resilient
```

### Why Reed-Solomon over RAID 6

| Feature | RAID 6 | Reed-Solomon (10,4) |
|---------|--------|---------------------|
| **Failure tolerance** | 2 disks | 4 nodes (any combination) |
| **Geographic distribution** | No | Yes (cross-datacenter) |
| **Scalability** | Limited by controller | Horizontal (thousands of nodes) |
| **Rebuild time** | Hours (GB/s) | Parallel (network-bound) |
| **Network awareness** | N/A | Topology-aware placement |
| **Cost** | Expensive RAID cards | Commodity servers |

---

## 2. Complete Datacenter-Grade Rust Tech Stack

### Core Dependencies

```toml
[workspace.dependencies]
# ===== Erasure Coding =====
reed-solomon-erasure = { version = "6.0", features = ["simd-accel"] }

# ===== Networking =====
libp2p = { version = "0.53", features = [
    "quic",              # QUIC for UDP-based transport
    "kad",               # Kademlia DHT for peer discovery
    "gossipsub",         # Pub/sub for cluster coordination
    "identify",          # Peer identification
    "ping",              # Liveness checking
    "noise",             # Noise protocol for encryption
    "yamux",             # Stream multiplexing
    "dns",               # DNS resolution
    "tcp",               # TCP fallback
    "macros",            # Ergonomic macros
] }
quinn = "0.10"           # Pure Rust QUIC implementation
rustls = "0.22"          # TLS 1.3 (no OpenSSL dependency)

# ===== Storage Backend =====
rocksdb = { version = "0.22", features = ["multi-threaded-cf"] }
sled = "0.34"            # Pure Rust alternative for metadata

# ===== Async Runtime =====
tokio = { version = "1.35", features = ["full", "tracing"] }
tokio-util = { version = "0.7", features = ["codec", "io"] }
tokio-stream = "0.1"

# ===== Cryptography =====
blake3 = { version = "1.5", features = ["rayon", "mmap"] }
aes-gcm = "0.10"
argon2 = "0.5"
ring = "0.17"

# ===== Serialization =====
prost = "0.12"           # Protobuf encoder/decoder
tonic = "0.11"           # gRPC framework
bincode = "1.3"          # Fast Rust-to-Rust serialization
serde = { version = "1.0", features = ["derive"] }

# ===== Database & Caching =====
sqlx = { version = "0.7", features = [
    "runtime-tokio-rustls",
    "postgres",
    "macros",
    "migrate",
] }
redis = { version = "0.24", features = ["tokio-comp", "cluster"] }

# ===== Observability =====
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
opentelemetry = { version = "0.21", features = ["rt-tokio"] }
metrics = "0.21"
metrics-exporter-prometheus = "0.13"

# ===== Performance =====
rayon = "1.8"            # Data parallelism
crossbeam = "0.8"        # Lock-free data structures
parking_lot = "0.12"     # Faster mutexes
memmap2 = "0.9"          # Memory-mapped files

# ===== Blockchain =====
solana-sdk = "1.18"
solana-client = "1.18"
```

### Technology Rationale

#### Networking: libp2p with QUIC

**Why libp2p**:
- Battle-tested: Used by IPFS, Filecoin, Polkadot
- DHT built-in: Kademlia for peer discovery without central registry
- NAT traversal: Hole punching, relays, AutoNAT
- Multi-transport: QUIC, TCP, WebSocket, WebRTC

**Why QUIC over TCP**:
```
Performance comparison (256KB chunk over 100ms RTT):
TCP + TLS 1.2: ~300ms (3 RTTs: SYN, TLS handshake, data)
QUIC + TLS 1.3: ~100ms (1 RTT with 0-RTT resumption)
```

#### Storage: RocksDB vs Sled

| Criteria | RocksDB | sled |
|----------|---------|------|
| **Maturity** | Production-proven | Experimental |
| **Performance** | 100K+ writes/sec | 50K writes/sec |
| **Production users** | Thousands | Dozens |
| **C++ dependency** | Yes | No (pure Rust) |

**Recommendation**: Hybrid approach
- RocksDB: Chunk data storage (high throughput, large values)
- sled: Metadata index (ACID transactions, simpler)

#### Crypto Stack

- **Blake3**: 10x faster than SHA-256 (SIMD parallelism)
- **AES-GCM**: Hardware AES-NI acceleration, authenticated encryption
- **ring**: Cryptographic primitives (ECDSA, etc.)

---

## 3. Cargo Workspace Structure

```
cyxcloud/
├── Cargo.toml                    # Workspace root
├── Cargo.lock                    # Pinned versions
├── rust-toolchain.toml           # Rust version (1.75.0)
│
├── cyxcloud-core/                # Core abstractions (no I/O)
│   └── src/
│       ├── lib.rs
│       ├── erasure.rs            # Reed-Solomon encoding/decoding
│       ├── crypto.rs             # Blake3, AES-GCM wrappers
│       ├── chunk.rs              # Chunk metadata types
│       └── error.rs              # Error types
│
├── cyxcloud-storage/             # Storage backend abstraction
│   └── src/
│       ├── lib.rs
│       ├── traits.rs             # StorageBackend trait
│       ├── rocksdb.rs            # RocksDB implementation
│       └── memory.rs             # In-memory (testing)
│
├── cyxcloud-network/             # libp2p networking
│   └── src/
│       ├── lib.rs
│       ├── behavior.rs           # libp2p NetworkBehaviour
│       ├── protocol.rs           # Custom protocols
│       ├── discovery.rs          # Peer discovery (Kademlia)
│       └── gossip.rs             # Gossipsub handlers
│
├── cyxcloud-metadata/            # Metadata service
│   └── src/
│       ├── lib.rs
│       ├── schema.rs             # Database schema (SQLx)
│       ├── index.rs              # File → Chunks mapping
│       └── cache.rs              # Redis caching layer
│
├── cyxcloud-node/                # Storage node daemon
│   └── src/
│       ├── main.rs               # Entry point
│       ├── config.rs             # Configuration (TOML)
│       ├── storage_manager.rs    # Local chunk storage
│       ├── replication.rs        # Chunk replication logic
│       └── health.rs             # Health checks
│
├── cyxcloud-gateway/             # S3-compatible API gateway
│   └── src/
│       ├── main.rs
│       ├── s3_api.rs             # S3 REST API (Axum)
│       └── auth.rs               # Authentication
│
├── cyxcloud-rebalancer/          # Data rebalancing service
│   └── src/
│       ├── main.rs
│       ├── detector.rs           # Failure detection
│       └── planner.rs            # Rebalancing algorithm
│
├── cyxcloud-cli/                 # Command-line client
│   └── src/
│       ├── main.rs
│       └── commands/
│
├── cyxcloud-protocol/            # Protobuf definitions
│   ├── build.rs
│   └── proto/
│       ├── chunk.proto
│       ├── metadata.proto
│       └── node.proto
│
└── benches/                      # Criterion benchmarks
    └── erasure_coding.rs
```

---

## 4. Performance Considerations

### Zero-Copy I/O Patterns

```rust
use memmap2::Mmap;

/// Zero-copy chunk reading using memory-mapped files
pub struct ChunkReader {
    mmap: Mmap,
    chunk_size: usize,
}

impl ChunkReader {
    /// Get chunk as a slice (no allocation!)
    pub fn chunk(&self, index: usize) -> &[u8] {
        let start = index * self.chunk_size;
        let end = (start + self.chunk_size).min(self.mmap.len());
        &self.mmap[start..end]
    }

    /// Hash chunk using SIMD-accelerated Blake3
    pub fn hash_chunk(&self, index: usize) -> blake3::Hash {
        blake3::hash(self.chunk(index))  // No copying!
    }
}
```

### Chunk Size Optimization

```rust
const MIN_CHUNK_SIZE: usize = 256 * 1024;        // 256 KB
const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;  // 4 MB
const MAX_CHUNK_SIZE: usize = 64 * 1024 * 1024;  // 64 MB

/// Determine optimal chunk size based on file size and network
pub fn optimal_chunk_size(
    file_size: u64,
    network_bandwidth_mbps: f64,
    network_latency_ms: f64,
) -> usize {
    // Bandwidth-Delay Product (BDP)
    let bdp_bytes = (network_bandwidth_mbps * 1_000_000.0 / 8.0)
        * (network_latency_ms / 1000.0);

    let target_chunks = (file_size as f64 / bdp_bytes)
        .clamp(10.0, 1000.0) as usize;

    let chunk_size = (file_size / target_chunks as u64) as usize;
    chunk_size.clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE)
}
```

### io_uring for Linux (Kernel Bypass I/O)

```rust
#[cfg(target_os = "linux")]
pub mod io_uring_storage {
    use tokio_uring::fs::File;

    /// High-performance chunk writer using io_uring
    pub async fn write_chunk_uring(
        path: &Path,
        data: &[u8],
    ) -> Result<()> {
        let file = File::create(path).await?;
        let (res, _) = file.write_at(data, 0).await;
        res?;
        file.sync_all().await?;
        Ok(())
    }
}
```

---

## 5. Datacenter-Specific Requirements

### Quorum-Based Reads/Writes

```rust
pub struct QuorumConfig {
    pub total_nodes: usize,       // 14
    pub read_quorum: usize,       // 10 (minimum to reconstruct)
    pub write_quorum: usize,      // 11 (majority)
    pub repair_threshold: usize,  // 12 (trigger repair)
}

impl QuorumConfig {
    pub fn datacenter_grade() -> Self {
        Self {
            total_nodes: 14,
            read_quorum: 10,   // Can tolerate 4 node failures
            write_quorum: 11,  // Ensures data durability
            repair_threshold: 12,
        }
    }
}
```

### Network Topology Awareness

```rust
#[derive(Debug, Clone)]
pub struct NodeLocation {
    pub datacenter: String,  // "us-east-1"
    pub rack: u32,           // Rack number
    pub node_id: NodeId,
}

/// Place shards with topology awareness
/// Target: Distribute across at least 3 datacenters
/// Constraint: No more than 6 shards in one datacenter
///
/// Example: 14 shards across 3 DCs:
/// DC1: 5 shards, DC2: 5 shards, DC3: 4 shards
/// Can survive loss of 1 entire datacenter
```

### Hardware Failure Detection

```rust
#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy,
    Degraded { reason: String },
    Failed { reason: String },
}

/// Multi-level health checks:
/// Layer 1: Network reachability (ICMP ping)
/// Layer 2: gRPC service health
/// Layer 3: Storage backend health (disk usage, IOPS)
```

### Hot-Swap Support

```rust
/// Graceful shutdown protocol for storage node
/// 1. Mark node as DRAINING (stop accepting new chunks)
/// 2. Get list of all chunks stored locally
/// 3. For each chunk, ensure redundancy exists elsewhere
/// 4. Mark node as OFFLINE
/// Safe to power off after drain complete
```

---

## 6. Performance Targets

| Metric | Target | Rationale |
|--------|--------|-----------|
| **Write throughput** | 1 GB/s per node | Saturate 10 Gbps network |
| **Read latency (P99)** | < 100 ms | 50ms network + 50ms processing |
| **Chunk encoding** | > 500 MB/s | Reed-Solomon with SIMD |
| **Chunk reconstruction** | > 1 GB/s | Parallel decoding |
| **Metadata lookup** | < 10 ms | RocksDB + Redis cache |
| **Failure detection** | < 30 seconds | Heartbeat interval |
| **Rebalancing rate** | 10 GB/hour/node | Background priority |

---

## 7. Implementation Phases

### Phase 1: Core Infrastructure (Weeks 1-4)
- [ ] Set up Cargo workspace with all crates
- [ ] Implement cyxcloud-core (erasure coding, crypto)
- [ ] Implement cyxcloud-storage (RocksDB backend)
- [ ] Write unit tests and benchmarks

### Phase 2: Networking (Weeks 5-8)
- [ ] Implement libp2p network layer
- [ ] Implement chunk transfer protocol
- [ ] Implement peer discovery and DHT
- [ ] Test on local cluster (3-5 nodes)

### Phase 3: Metadata & Coordination (Weeks 9-12)
- [ ] Implement cyxcloud-metadata (PostgreSQL schema)
- [ ] Implement quorum-based reads/writes
- [ ] Implement topology-aware node selection
- [ ] Implement health checking

### Phase 4: Production Features (Weeks 13-16)
- [ ] Implement cyxcloud-gateway (S3-compatible API)
- [ ] Implement cyxcloud-rebalancer
- [ ] Implement graceful shutdown and hot-swap
- [ ] Performance testing (1 GB/s target)

### Phase 5: Integration & Deployment (Weeks 17-20)
- [ ] Integrate with CyxWiz Engine (C++ client)
- [ ] Integrate with CyxWiz Central Server
- [ ] Implement Solana payment integration
- [ ] Deploy to testnet (100+ nodes)

---

## 8. Open Questions

1. **Storage economics**: Target $/TB/month vs AWS S3 ($23/TB)?
2. **Geographic distribution**: How many regions for initial launch?
3. **Hardware specs**: Minimum server requirements?
4. **Blockchain settlement**: Real-time micropayments or daily batching?
5. **Compliance**: GDPR, data sovereignty, encryption key escrow?
