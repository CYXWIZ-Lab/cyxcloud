# CyxCloud Usage Guide

This guide explains how to build, run, and use the CyxCloud decentralized storage platform.

## Table of Contents

1. [Network Architecture](#network-architecture)
2. [Blockchain Integration](#blockchain-integration)
3. [Prerequisites](#prerequisites)
4. [Building from Source](#building-from-source)
5. [Running Components](#running-components)
6. [CLI Usage](#cli-usage)
7. [Docker Deployment](#docker-deployment)
8. [API Reference](#api-reference)
9. [Configuration](#configuration)
10. [Troubleshooting](#troubleshooting)

---

## Network Architecture

CyxCloud is a decentralized storage platform that distributes data across multiple nodes using erasure coding for redundancy. This section explains the system architecture and data flow.

### System Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CyxCloud Network                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐         ┌──────────────┐         ┌──────────────┐         │
│  │   Client     │  HTTP   │   Gateway    │  gRPC   │    Nodes     │         │
│  │  (CLI/SDK)   │────────▶│  (S3 API)    │────────▶│  (Storage)   │         │
│  └──────────────┘         └──────────────┘         └──────────────┘         │
│                                  │                        │                  │
│                                  │ SQL                    │ libp2p           │
│                                  ▼                        ▼                  │
│                           ┌──────────────┐         ┌──────────────┐         │
│                           │  PostgreSQL  │         │  Kademlia    │         │
│                           │  (Metadata)  │         │  DHT (P2P)   │         │
│                           └──────────────┘         └──────────────┘         │
│                                  │                                           │
│                                  │                                           │
│                           ┌──────────────┐                                  │
│                           │    Redis     │                                  │
│                           │   (Cache)    │                                  │
│                           └──────────────┘                                  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Components

| Component | Role | Technology |
|-----------|------|------------|
| **Gateway** | S3-compatible HTTP API, request routing | Axum, Tower |
| **Storage Nodes** | Store/retrieve chunks, peer discovery | gRPC (Tonic), libp2p |
| **Rebalancer** | Monitor chunk health, repair data | Background service |
| **PostgreSQL** | File/chunk metadata, node registry | SQLx |
| **Redis** | Hot data cache, session state | Redis crate |

### Hybrid Networking: gRPC + libp2p

CyxCloud uses a hybrid networking approach:

```
┌─────────────────────────────────────────────────────────────────┐
│                     Networking Layer                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌─────────────────────────┐    ┌─────────────────────────┐    │
│   │        gRPC             │    │        libp2p           │    │
│   │   (Data Transfer)       │    │    (Peer Discovery)     │    │
│   ├─────────────────────────┤    ├─────────────────────────┤    │
│   │ • Chunk upload/download │    │ • Kademlia DHT          │    │
│   │ • Streaming transfers   │    │ • Node announcements    │    │
│   │ • Health checks         │    │ • Peer routing          │    │
│   │ • TLS encryption        │    │ • NAT traversal         │    │
│   │                         │    │                         │    │
│   │ Port: 50051 (default)   │    │ Port: 4001 (default)    │    │
│   └─────────────────────────┘    └─────────────────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Why hybrid?**
- **gRPC**: Optimized for large data transfers, streaming, connection pooling
- **libp2p**: Decentralized peer discovery without central registry

### Erasure Coding (Not RAID)

CyxCloud uses **Reed-Solomon erasure coding** for data redundancy, not traditional RAID:

```
                        Original File (64 MB)
                               │
                               ▼
                    ┌─────────────────────┐
                    │   Split into chunks │
                    │     (64 MB each)    │
                    └─────────────────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │   Erasure Encoding  │
                    │  (10 data + 4 parity)│
                    └─────────────────────┘
                               │
       ┌───────┬───────┬───────┼───────┬───────┬───────┐
       ▼       ▼       ▼       ▼       ▼       ▼       ▼
    ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐
    │ S1  │ │ S2  │ │ S3  │ │...  │ │ S12 │ │ S13 │ │ S14 │
    │Data │ │Data │ │Data │ │     │ │Pari │ │Pari │ │Pari │
    └─────┘ └─────┘ └─────┘ └─────┘ └─────┘ └─────┘ └─────┘
       │       │       │       │       │       │       │
       ▼       ▼       ▼       ▼       ▼       ▼       ▼
    ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐
    │NodeA│ │NodeB│ │NodeC│ │ ... │ │NodeL│ │NodeM│ │NodeN│
    └─────┘ └─────┘ └─────┘ └─────┘ └─────┘ └─────┘ └─────┘
```

**Key Properties:**
- **14 shards total**: 10 data + 4 parity
- **Reconstruction**: Any 10 of 14 shards can rebuild original data
- **Fault tolerance**: Survives up to 4 simultaneous node failures
- **Storage overhead**: 40% (4/10 = 0.4x extra storage)

### Erasure Coding vs RAID Comparison

| Feature | CyxCloud (Erasure Coding) | Traditional RAID |
|---------|---------------------------|------------------|
| **Scope** | Distributed across network | Single machine |
| **Fault Tolerance** | 4 node failures (default) | 1-2 disk failures |
| **Storage Overhead** | 40% (configurable) | 12.5%-100% |
| **Rebuild Speed** | Parallel from any nodes | Sequential, single disk |
| **Geographic** | Cross-datacenter possible | Single location |
| **Scalability** | Add nodes dynamically | Limited by controller |

### Redundancy Configurations

| Profile | Data Shards | Parity Shards | Can Lose | Overhead | Use Case |
|---------|-------------|---------------|----------|----------|----------|
| **Default** | 10 | 4 | 4 nodes | 40% | General purpose |
| **High Availability** | 8 | 6 | 6 nodes | 75% | Important data |
| **Maximum HA** | 8 | 8 | 8 nodes | 100% | Critical data |
| **Extreme HA** | 6 | 10 | 10 nodes | 167% | Financial/medical |

### Data Flow: Upload

```
┌────────┐     ┌─────────┐     ┌──────────┐     ┌───────────┐
│ Client │────▶│ Gateway │────▶│ Chunker  │────▶│  Erasure  │
└────────┘     └─────────┘     └──────────┘     │  Encoder  │
                                                └───────────┘
                                                      │
                    ┌─────────────────────────────────┘
                    ▼
           ┌───────────────┐
           │ Node Selector │ ◀─── Latency-based selection
           │ (Topology)    │ ◀─── Rack/DC awareness
           └───────────────┘
                    │
        ┌───────────┼───────────┬───────────┐
        ▼           ▼           ▼           ▼
    ┌───────┐   ┌───────┐   ┌───────┐   ┌───────┐
    │Node 1 │   │Node 2 │   │Node 3 │   │Node N │
    │Shard 1│   │Shard 2│   │Shard 3│   │Shard N│
    └───────┘   └───────┘   └───────┘   └───────┘
        │           │           │           │
        └───────────┴───────────┴───────────┘
                          │
                          ▼
                 ┌─────────────────┐
                 │   PostgreSQL    │
                 │ (chunk → nodes) │
                 └─────────────────┘
```

**Upload Steps:**
1. Client uploads file to Gateway via HTTP
2. Gateway splits file into 64 MB chunks
3. Each chunk is erasure-encoded into 14 shards (10 data + 4 parity)
4. Node Selector picks 14 nodes (topology-aware)
5. Shards are transferred via gRPC in parallel
6. Metadata stored in PostgreSQL

### Data Flow: Download

```
┌────────┐     ┌─────────┐     ┌──────────────┐
│ Client │────▶│ Gateway │────▶│  PostgreSQL  │
└────────┘     └─────────┘     │ (lookup nodes)│
                    │          └──────────────┘
                    │                 │
                    │     ┌───────────┘
                    ▼     ▼
           ┌───────────────────┐
           │  Parallel Fetch   │
           │ (fastest 10 of 14)│
           └───────────────────┘
                    │
        ┌───────────┼───────────┐
        ▼           ▼           ▼
    ┌───────┐   ┌───────┐   ┌───────┐
    │Node A │   │Node B │   │Node C │   ... (fetch from 10+ nodes)
    │Shard 1│   │Shard 3│   │Shard 7│
    └───────┘   └───────┘   └───────┘
        │           │           │
        └───────────┴───────────┘
                    │
                    ▼
           ┌───────────────┐
           │    Erasure    │
           │    Decoder    │
           └───────────────┘
                    │
                    ▼
           ┌───────────────┐
           │ Original Data │
           └───────────────┘
```

**Download Steps:**
1. Client requests file from Gateway
2. Gateway queries PostgreSQL for chunk locations
3. Gateway fetches shards from fastest 10+ nodes (parallel)
4. Erasure decoder reconstructs original chunk
5. Chunks are streamed back to client

### Rebalancer Service

The rebalancer runs continuously to maintain data health:

```
┌─────────────────────────────────────────────────────────────┐
│                    Rebalancer Service                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌────────────┐    ┌────────────┐    ┌────────────┐         │
│  │  Detector  │───▶│  Planner   │───▶│  Executor  │         │
│  └────────────┘    └────────────┘    └────────────┘         │
│        │                 │                 │                 │
│        ▼                 ▼                 ▼                 │
│  • Scan for issues  • Prioritize     • Parallel repair     │
│  • Under-replicated • Select targets • Rate limiting       │
│  • Corrupt shards   • Avoid hotspots • Retry logic         │
│  • Orphaned chunks  • Balance load   • Progress tracking   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Detected Issues:**
- **Under-replicated**: Fewer than 14 shards available
- **Over-replicated**: More than 14 shards (cleanup)
- **Corrupt**: Checksum mismatch
- **Orphaned**: Shards with no metadata reference

### Topology-Aware Placement

CyxCloud distributes shards across failure domains:

```
┌─────────────────────────────────────────────────────────────┐
│                    Datacenter: US-East                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────┐         ┌─────────────────┐            │
│  │    Rack A       │         │    Rack B       │            │
│  ├─────────────────┤         ├─────────────────┤            │
│  │ Node-1: S1, S5  │         │ Node-4: S2, S6  │            │
│  │ Node-2: S9, S13 │         │ Node-5: S10, S14│            │
│  │ Node-3: ...     │         │ Node-6: ...     │            │
│  └─────────────────┘         └─────────────────┘            │
│                                                              │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    Datacenter: US-West                       │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────┐         ┌─────────────────┐            │
│  │    Rack C       │         │    Rack D       │            │
│  ├─────────────────┤         ├─────────────────┤            │
│  │ Node-7: S3, S7  │         │ Node-10: S4, S8 │            │
│  │ Node-8: S11     │         │ Node-11: S12    │            │
│  │ Node-9: ...     │         │ Node-12: ...    │            │
│  └─────────────────┘         └─────────────────┘            │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Placement Rules:**
- Max 2 shards per node
- Max 4 shards per rack
- Max 6 shards per datacenter
- Prefer low-latency nodes for data shards

### Security Model

```
┌─────────────────────────────────────────────────────────────┐
│                      Security Layers                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────┐                                        │
│  │   Transport     │  TLS 1.3 for all connections           │
│  └─────────────────┘                                        │
│           │                                                  │
│  ┌─────────────────┐                                        │
│  │  Authentication │  JWT tokens, API keys                  │
│  └─────────────────┘                                        │
│           │                                                  │
│  ┌─────────────────┐                                        │
│  │   Encryption    │  AES-256-GCM at rest (optional)        │
│  └─────────────────┘                                        │
│           │                                                  │
│  ┌─────────────────┐                                        │
│  │    Integrity    │  Blake3 checksums per shard            │
│  └─────────────────┘                                        │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### Storage Architecture

Each storage node uses a local storage backend for chunk persistence:

```
┌─────────────────────────────────────────────────────────────┐
│                    Storage Node                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                   gRPC Server                        │    │
│  │              (ChunkService impl)                     │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                   │
│                          ▼                                   │
│  ┌─────────────────────────────────────────────────────┐    │
│  │               Storage Backend                        │    │
│  │         (RocksDB / Sled / Memory)                   │    │
│  └─────────────────────────────────────────────────────┘    │
│                          │                                   │
│         ┌────────────────┼────────────────┐                 │
│         ▼                ▼                ▼                 │
│  ┌────────────┐   ┌────────────┐   ┌────────────┐          │
│  │   Chunks   │   │  Metadata  │   │   Stats    │          │
│  │  (binary)  │   │   (JSON)   │   │ (counters) │          │
│  └────────────┘   └────────────┘   └────────────┘          │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                 Local Disk                           │    │
│  │              /data/chunks/                           │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### Storage Backends

| Backend | Use Case | Persistence | Performance |
|---------|----------|-------------|-------------|
| **RocksDB** | Production | Yes | High (LSM tree) |
| **Sled** | Embedded | Yes | Medium (B+ tree) |
| **Memory** | Testing | No | Very High |

#### RocksDB Storage Layout

```
/data/chunks/
├── rocksdb/
│   ├── 000003.log           # Write-ahead log
│   ├── 000004.sst           # SSTable (sorted data)
│   ├── 000005.sst
│   ├── MANIFEST-000001      # Database manifest
│   ├── CURRENT              # Current manifest pointer
│   ├── OPTIONS-000001       # RocksDB options
│   └── LOCK                 # Database lock
└── metadata.json            # Node metadata
```

#### Chunk Storage Format

Each shard is stored as a key-value pair:

```
┌─────────────────────────────────────────────────────────────┐
│                      Chunk Record                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Key (32 bytes):                                            │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Blake3 Hash of Chunk Content (256-bit)              │   │
│  │  Example: a1b2c3d4e5f6...                            │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│  Value (variable):                                          │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  [4 bytes]  Magic number (0x43595843 = "CYXC")       │   │
│  │  [4 bytes]  Version (1)                               │   │
│  │  [4 bytes]  Flags (encrypted, compressed)            │   │
│  │  [8 bytes]  Original size                             │   │
│  │  [8 bytes]  Stored size                               │   │
│  │  [32 bytes] Content hash (Blake3)                    │   │
│  │  [N bytes]  Chunk data (possibly encrypted/compressed)│   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### Sled Metadata Schema

The Sled backend stores file and chunk metadata:

```
┌─────────────────────────────────────────────────────────────┐
│                    Metadata Trees                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  files:                                                     │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Key: file_id (UUID)                                 │   │
│  │  Value: {                                            │   │
│  │    "name": "dataset.csv",                            │   │
│  │    "size": 1073741824,                               │   │
│  │    "chunk_count": 16,                                │   │
│  │    "created_at": "2024-01-15T10:30:00Z",             │   │
│  │    "content_type": "text/csv",                       │   │
│  │    "erasure_config": { "data": 8, "parity": 6 }      │   │
│  │  }                                                   │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│  chunks:                                                    │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Key: chunk_id (Blake3 hash)                         │   │
│  │  Value: {                                            │   │
│  │    "file_id": "uuid-here",                           │   │
│  │    "index": 0,                                       │   │
│  │    "size": 67108864,                                 │   │
│  │    "shard_index": 3,                                 │   │
│  │    "locations": ["node-1", "node-5", "node-9"]       │   │
│  │  }                                                   │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
│  locations:                                                 │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Key: chunk_id:node_id                               │   │
│  │  Value: {                                            │   │
│  │    "stored_at": "2024-01-15T10:30:00Z",              │   │
│  │    "verified_at": "2024-01-15T12:00:00Z",            │   │
│  │    "status": "healthy"                               │   │
│  │  }                                                   │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### Storage Metrics

Each storage backend tracks performance metrics:

```
┌─────────────────────────────────────────────────────────────┐
│                    Storage Metrics                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  Capacity:                                                  │
│  ├── Total:      100 GB                                     │
│  ├── Used:       45.2 GB (45.2%)                           │
│  └── Available:  54.8 GB                                    │
│                                                              │
│  Chunks:                                                    │
│  ├── Count:      12,847                                     │
│  ├── Avg Size:   3.6 MB                                     │
│  └── Total:      45.2 GB                                    │
│                                                              │
│  Latency (microseconds):                                    │
│  ├── Read Avg:   1,250 μs                                   │
│  ├── Read P99:   5,800 μs                                   │
│  ├── Write Avg:  2,100 μs                                   │
│  └── Write P99:  8,500 μs                                   │
│                                                              │
│  Throughput:                                                │
│  ├── Read:       850 MB/s                                   │
│  └── Write:      420 MB/s                                   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### Encryption at Rest

When encryption is enabled, chunks are encrypted before storage:

```
┌─────────────────────────────────────────────────────────────┐
│                  Encryption Pipeline                         │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐              │
│  │ Plaintext│───▶│  Blake3  │───▶│  AES-256 │───▶ Stored   │
│  │  Chunk   │    │  Hash    │    │   GCM    │              │
│  └──────────┘    └──────────┘    └──────────┘              │
│                       │               │                     │
│                       ▼               ▼                     │
│                  Content ID      Encrypted                  │
│                  (chunk key)     Chunk Data                 │
│                                                              │
│  Key Derivation:                                            │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Master Key (user-provided or derived from password) │   │
│  │       │                                               │   │
│  │       ▼                                               │   │
│  │  HKDF-SHA256(master_key, chunk_id) → chunk_key       │   │
│  │       │                                               │   │
│  │       ▼                                               │   │
│  │  AES-256-GCM(chunk_key, nonce, data) → ciphertext    │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### Garbage Collection

Orphaned chunks are cleaned up periodically:

```
┌─────────────────────────────────────────────────────────────┐
│                  Garbage Collection                          │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  1. Mark Phase:                                             │
│     - Scan all file metadata                                │
│     - Build set of referenced chunk IDs                     │
│                                                              │
│  2. Sweep Phase:                                            │
│     - Iterate all stored chunks                             │
│     - Delete chunks not in reference set                    │
│     - Rate-limited to avoid I/O spikes                      │
│                                                              │
│  3. Compaction:                                             │
│     - Trigger RocksDB compaction                            │
│     - Reclaim disk space                                    │
│                                                              │
│  Schedule: Daily at 3:00 AM (configurable)                  │
│  Rate Limit: 100 MB/s deletion                              │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## Blockchain Integration

CyxCloud uses Solana blockchain for decentralized payments, subscriptions, and node staking. All programs are deployed and operational.

### Deployed Solana Programs (Devnet)

| Program | Address | Purpose |
|---------|---------|---------|
| **StorageSubscription** | `HZhWDJVkkUuHrgqkNb9bYxiCfqtnv8cnus9UQt843Fro` | User storage plans (Free/Starter/Pro/Enterprise) |
| **StorageNodeRegistry** | `AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z` | Node registration, staking, proof-of-storage |
| **StoragePaymentPool** | `4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX` | Weekly reward distribution (85/10/5 split) |
| **CYXWIZ Token** | `Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi` | SPL Token (9 decimals) |

### Payment Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Payment Flow                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  User                     Gateway                  Blockchain   │
│  ┌────┐                  ┌────────┐              ┌──────────┐  │
│  │Pay │──CYXWIZ tokens──▶│ Pool   │──deposit────▶│PaymentPool│  │
│  │    │                  │        │              │           │  │
│  └────┘                  └────────┘              └──────────┘  │
│                                                       │         │
│                          Weekly Epoch                 │         │
│                               │                       │         │
│                               ▼                       ▼         │
│                    ┌─────────────────────────────────────┐     │
│                    │         Distribution                 │     │
│                    │  85% → Storage Nodes                │     │
│                    │  10% → Platform Treasury            │     │
│                    │   5% → Community Fund               │     │
│                    └─────────────────────────────────────┘     │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Node Staking

Storage nodes must stake CYXWIZ tokens to participate:

| Requirement | Value |
|-------------|-------|
| **Minimum Stake** | 500 CYXWIZ |
| **Unstake Lockup** | 7 days |
| **Heartbeat Timeout** | 5 minutes |

**Slashing Penalties:**
| Violation | Stake Penalty |
|-----------|---------------|
| Failed Proofs (3 consecutive) | 15% |
| Extended Downtime | 5% |
| Data Loss | 10% |
| Corrupted Data | 50% |

### Proof-of-Storage

Nodes must prove they still hold data through challenge-response:

```
Gateway                    Storage Node                 Blockchain
   │                            │                            │
   │── 1. Challenge ───────────▶│                            │
   │   (chunk_id, nonce)        │                            │
   │                            │                            │
   │                            │── 2. Compute proof ────────│
   │                            │   hash(chunk_data + nonce) │
   │                            │                            │
   │                            │── 3. submit_proof() ──────▶│
   │                            │                            │
   │◀── 4. Verify ──────────────│◀── result ─────────────────│
```

### Subscription Tiers

| Plan | Storage | Monthly Price | Yearly (20% off) |
|------|---------|---------------|------------------|
| **Free** | 5 GB | 0 CYXWIZ | - |
| **Starter** | 100 GB | 10 CYXWIZ | 96 CYXWIZ |
| **Pro** | 1 TB | 50 CYXWIZ | 480 CYXWIZ |
| **Enterprise** | 10 TB | 200 CYXWIZ | 1,920 CYXWIZ |

### Blockchain Client Integration

**Gateway** (`cyxcloud-gateway/src/blockchain/`):
- Manages subscriptions and usage tracking
- Updates node metrics on-chain
- Initiates weekly payment epochs
- Slashes misbehaving nodes

**Storage Node** (`cyxcloud-node/src/blockchain/`):
- Registers and stakes tokens
- Sends heartbeats with proof-of-storage
- Claims weekly rewards

### Environment Variables for Blockchain

```bash
# RPC endpoint
export SOLANA_RPC_URL="https://api.devnet.solana.com"

# Keypair for signing transactions
export GATEWAY_KEYPAIR_PATH="~/.config/solana/id.json"

# Program IDs (auto-configured for devnet)
export SUBSCRIPTION_PROGRAM_ID="HZhWDJVkkUuHrgqkNb9bYxiCfqtnv8cnus9UQt843Fro"
export NODE_REGISTRY_PROGRAM_ID="AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z"
export PAYMENT_POOL_PROGRAM_ID="4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX"
export CYXWIZ_TOKEN_MINT="Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi"
```

---

## Prerequisites

### Required Software

- **Rust 1.75+** - Install from https://rustup.rs
- **Protobuf Compiler** - For gRPC code generation
  ```bash
  # Windows (with chocolatey)
  choco install protoc

  # macOS
  brew install protobuf

  # Ubuntu/Debian
  sudo apt install protobuf-compiler
  ```

### Optional (for Docker deployment)

- **Docker** - https://docs.docker.com/get-docker/
- **Docker Compose** - Usually included with Docker Desktop

---

## Building from Source

### Build All Components

```bash
cd cyx_cloud

# Debug build (faster compile, slower runtime)
cargo build --workspace

# Release build (slower compile, optimized runtime)
cargo build --workspace --release
```

### Build Individual Components

```bash
# Gateway only
cargo build -p cyxcloud-gateway --release

# Storage node only
cargo build -p cyxcloud-node --release

# CLI only
cargo build -p cyxcloud-cli --release

# Rebalancer only
cargo build -p cyxcloud-rebalancer --release
```

### Run Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p cyxcloud-core
cargo test -p cyxcloud-network

# With output
cargo test --workspace -- --nocapture
```

### Run Benchmarks

```bash
# Erasure coding benchmarks
cargo bench -p cyxcloud-core

# Storage benchmarks
cargo bench -p cyxcloud-storage
```

---

## Running Components

### 1. Start the Gateway

The gateway provides the S3-compatible HTTP API.

```bash
# Using cargo
cargo run -p cyxcloud-gateway --release

# Or run the binary directly
./target/release/cyxcloud-gateway
```

**Default Configuration:**
- Host: `0.0.0.0`
- Port: `8080`
- Health endpoint: `http://localhost:8080/health`

**Environment Variables:**
```bash
export RUST_LOG=info           # Logging level (trace, debug, info, warn, error)
export GATEWAY_HOST=0.0.0.0    # Bind address
export GATEWAY_PORT=8080       # HTTP port
```

### 2. Start Storage Nodes (cyxcloud-node)

Storage nodes store and serve chunks for the CyxCloud network. They are the backbone of the decentralized storage system.

#### Quick Start

```bash
# Build the storage node
cargo build -p cyxcloud-node --release

# Run the binary
./target/release/cyxcloud-node
```

#### Authentication Flow

Storage node operators must authenticate with their CyxWiz account. On first run, the node prompts for login:

```
$ ./cyxcloud-node

2024-12-22T10:00:00  INFO  CyxCloud Storage Node starting...
2024-12-22T10:00:00  INFO  Configuration loaded

╔══════════════════════════════════════════════════════════════╗
║           CyxWiz Storage Node - Login Required               ║
╚══════════════════════════════════════════════════════════════╝

Please login with your CyxWiz account to register this node.

Email: miner@example.com
Password: ••••••••          ← Hidden input

Logging in...

✓ Login successful!
  Welcome, John Doe!

Registering machine...
✓ Machine registered successfully!

========================================
  Machine service running
  Sending heartbeats every 30 seconds
========================================
```

**Authentication Process:**
1. Node prompts for email and password (password hidden)
2. Authenticates via CyxWiz REST API (`POST /api/auth/login`)
3. Receives JWT token and user info
4. Registers machine with `POST /api/machines/register`
5. Receives machine ID and API key
6. Saves credentials to `{data_dir}/.cyxwiz_credentials.json`

**Subsequent Runs:**
```
$ ./cyxcloud-node

2024-12-22T10:00:00  INFO  CyxCloud Storage Node starting...
2024-12-22T10:00:00  INFO  Loaded existing credentials - machine already registered
2024-12-22T10:00:00  INFO  Machine service running
```

Credentials are saved locally, so you only need to login once.

**Logout / Re-login:**
```bash
# Delete saved credentials to login again
rm ./data/.cyxwiz_credentials.json
./cyxcloud-node   # Will prompt for login
```

#### Storage Allocation

Storage is configured via the config file or command-line options:

```bash
# Run with default 100 GB allocation
./cyxcloud-node

# Run with custom data directory
./cyxcloud-node --data-dir /mnt/storage/cyxcloud

# Run with custom config file
./cyxcloud-node --config /etc/cyxcloud/node.toml
```

**Registration Process:**
1. Node authenticates with CyxWiz API (email/password)
2. Node registers machine with detected hardware info (CPU, GPU, RAM)
3. CyxWiz API creates machine record and returns API key
4. Node sends heartbeats every 30 seconds with:
   - CPU load percentage
   - Storage used/available
   - Online status
5. Miner earns CYXWIZ tokens for storage provided

#### Running Multiple Nodes

```bash
# Node 1 (Terminal 1)
NODE_ID=node-1 GRPC_PORT=50051 LIBP2P_PORT=4001 \
  ./target/release/cyxcloud-node --data-dir /data/node1

# Node 2 (Terminal 2)
NODE_ID=node-2 GRPC_PORT=50052 LIBP2P_PORT=4002 \
  ./target/release/cyxcloud-node --data-dir /data/node2

# Node 3 (Terminal 3)
NODE_ID=node-3 GRPC_PORT=50053 LIBP2P_PORT=4003 \
  ./target/release/cyxcloud-node --data-dir /data/node3
```

#### Configuration File

Nodes can be configured via `config.toml`:

```toml
[node]
id = "node-1"                        # Unique node identifier
name = "my-mining-rig"               # Human-readable name
region = "us-east"                   # Region for topology placement
wallet_address = "CyxWiz..."         # Solana wallet for payments

[storage]
data_dir = "./data"                  # Data directory (credentials saved here)
max_capacity_gb = 100                # Maximum storage allocation
compression = true                   # Enable LZ4 compression
cache_size_mb = 512                  # RocksDB cache size

[network]
bind_address = "0.0.0.0"
grpc_port = 50051                    # gRPC server for chunk operations
p2p_port = 4001                      # libp2p peer discovery
enable_tls = false

[central]
address = "http://localhost:50052"   # Gateway gRPC address
register = true                      # Register with Gateway
heartbeat_interval_secs = 30         # Heartbeat frequency

[cyxwiz_api]
base_url = "http://localhost:3002"   # CyxWiz REST API for auth
register = true                      # Enable machine registration
# Credentials are saved after first login, no need to configure:
# owner_id, api_key, machine_id

[metrics]
enabled = true
port = 9090                          # Prometheus metrics endpoint
health_path = "/health"
metrics_path = "/metrics"
```

#### Environment Variables

```bash
# Node identity
export NODE_ID=node-1                    # Unique node identifier
export NODE_NAME="my-mining-rig"         # Human-readable name

# Network settings
export GRPC_HOST=0.0.0.0                 # gRPC bind address
export GRPC_PORT=50051                   # gRPC port for chunk transfer
export LIBP2P_PORT=4001                  # libp2p port for peer discovery

# Gateway settings (for chunk storage)
export CENTRAL_ADDRESS=http://localhost:50052  # Gateway gRPC address

# CyxWiz API settings (for authentication)
export CYXWIZ_API_URL=http://localhost:3002    # CyxWiz REST API
# Note: Credentials saved automatically after login

# Storage settings
export STORAGE_PATH=./data               # Data directory
export STORAGE_CAPACITY_GB=100           # Storage allocation

# Location (for topology-aware placement)
export NODE_REGION=us-east
export NODE_DATACENTER=dc1

# Logging
export RUST_LOG=info                     # Log level (trace/debug/info/warn/error)
```

#### Node Lifecycle

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Login     │────▶│  Allocate   │────▶│   Running   │
└─────────────┘     └─────────────┘     └─────────────┘
                                              │
    ┌─────────────┐     ┌─────────────┐       │
    │   Drained   │◀────│  Draining   │◀──────┘
    └─────────────┘     └─────────────┘

Running Node Services:
├── gRPC Server (ChunkService)     - Store/retrieve chunks
├── libp2p Swarm                   - Peer discovery via Kademlia DHT
├── Heartbeat Service              - Report health to Gateway every 30s
└── Metrics Endpoint               - Prometheus metrics on :9090
```

#### Heartbeat & Health

Nodes send heartbeats to the Gateway every 30 seconds:

```
Node ──gRPC + JWT──▶ Gateway
                         │
                         ├─ Update last_heartbeat
                         ├─ Report storage_used
                         ├─ Report active connections
                         └─ Check for pending commands
                              (repair, transfer, delete)
```

If a node misses 3 consecutive heartbeats, it's marked as `OFFLINE` and chunks may be replicated to other nodes.

#### Graceful Shutdown

```bash
# Signal the node to drain (stop accepting new chunks)
cyxcloud-node --drain

# Or send SIGTERM
kill -SIGTERM <pid>
```

During draining:
1. Node stops accepting new chunk storage requests
2. Existing chunks remain accessible
3. Rebalancer transfers chunks to other nodes
4. Once empty, node can safely shut down

#### Storage Miner Earnings

Operators earn CYXWIZ tokens for providing storage:

```
Every billing period (hourly):

Gateway calculates:
├── storage_provided (GB-hours)
├── bandwidth_served (GB transferred)
├── uptime_percentage

Earnings = base_rate × storage + bandwidth_rate × bandwidth
Payment: platform_wallet → operator_wallet (via Solana)

Example: 100 GB × 24 hours × 0.001 CYXWIZ = 2.4 CYXWIZ/day
```

#### Monitoring

```bash
# Check node status
curl http://localhost:9090/metrics

# Key metrics:
# cyxcloud_storage_used_bytes     - Current storage usage
# cyxcloud_chunks_stored          - Number of chunks stored
# cyxcloud_bytes_uploaded         - Total bytes received
# cyxcloud_bytes_downloaded       - Total bytes served
# cyxcloud_active_connections     - Current gRPC connections
```

### 3. Start the Rebalancer (Optional)

The rebalancer monitors chunk health and repairs under-replicated data.

```bash
cargo run -p cyxcloud-rebalancer --release
```

**Environment Variables:**
```bash
export SCAN_INTERVAL_SECS=3600         # Time between scans (1 hour)
export REPAIR_PARALLELISM=4            # Concurrent repair tasks
export RATE_LIMIT_BYTES_PER_HOUR=10737418240  # 10 GB/hour rate limit
```

---

## CLI Usage

The `cyxcloud` CLI provides a command-line interface for interacting with CyxCloud storage.

### Installation

```bash
# Build and install
cargo install --path cyxcloud-cli

# Or run directly
cargo run -p cyxcloud-cli --release -- <command>
```

### Global Options

```bash
cyxcloud --gateway http://localhost:8080 <command>
```

### Upload Files

```bash
# Upload a single file
cyxcloud upload ./myfile.txt -b mybucket

# Upload with custom key prefix
cyxcloud upload ./myfile.txt -b mybucket -p data/uploads

# Upload a directory recursively
cyxcloud upload ./my-folder -b mybucket

# Upload with encryption (not yet implemented)
cyxcloud upload ./sensitive.txt -b mybucket -e
```

**Options:**
- `-b, --bucket <NAME>` - Target bucket (default: "default")
- `-p, --prefix <PREFIX>` - Key prefix for uploaded files
- `-e, --encrypt` - Enable encryption (placeholder)

### Download Files

```bash
# Download a specific file
cyxcloud download mybucket -k myfile.txt -o ./downloads/

# Download to current directory
cyxcloud download mybucket -k data/file.txt

# Download all files with prefix
cyxcloud download mybucket --prefix data/ -o ./backup/

# Download entire bucket
cyxcloud download mybucket -o ./full-backup/
```

**Options:**
- `-k, --key <KEY>` - Specific object key to download
- `--prefix <PREFIX>` - Download all objects with this prefix
- `-o, --output <PATH>` - Output path (default: current directory)

### List Objects

```bash
# List all objects in bucket
cyxcloud list mybucket

# Long format with details
cyxcloud list mybucket -l

# Human-readable sizes
cyxcloud list mybucket -l -H

# Filter by prefix
cyxcloud list mybucket -p data/

# Limit results
cyxcloud list mybucket --max-keys 100
```

**Options:**
- `-p, --prefix <PREFIX>` - Filter by key prefix
- `-l, --long` - Show detailed information (size, date, etag)
- `-H, --human-readable` - Human-readable sizes (KB, MB, GB)
- `--max-keys <N>` - Maximum number of keys to return

**Example Output (long format):**
```
KEY                                      SIZE        LAST MODIFIED            ETAG
----------------------------------------------------------------------------------------------------
data/file1.txt                           1.50 KB     2024-01-15T10:30:00Z     abc123...
data/file2.txt                           2.30 MB     2024-01-15T11:45:00Z     def456...
images/photo.jpg                         4.50 MB     2024-01-14T09:00:00Z     ghi789...
----------------------------------------------------------------------------------------------------
3 objects, 6.82 MB total
```

### Delete Objects

```bash
# Preview deletion (shows what will be deleted)
cyxcloud delete mybucket -k data/old-file.txt

# Delete with confirmation bypass
cyxcloud delete mybucket -k data/old-file.txt --force
```

**Options:**
- `-k, --key <KEY>` - Object key to delete (required)
- `-f, --force` - Skip confirmation and delete immediately

**Example Output (without --force):**
```
Warning: About to delete: mybucket/data/old-file.txt
  Size: 1024 bytes
  Last modified: 2024-01-15T10:30:00Z

Use --force to delete without confirmation.
```

**Example Output (with --force):**
```
✓ Deleted: mybucket/data/old-file.txt
```

### Check Status

```bash
# Overall gateway status
cyxcloud status

# Specific bucket status
cyxcloud status -b mybucket

# Verbose output
cyxcloud status -b mybucket -v
```

**Options:**
- `-b, --bucket <NAME>` - Show specific bucket status
- `-v, --verbose` - Show detailed information

**Example Output:**
```
CyxCloud Storage Status

Gateway Status: ● Online

Bucket: mybucket

  Objects:      42
  Total Size:   1.23 GB

  Recent Objects:
    data/latest.csv (45.2 KB) - 2024-01-15T12:00:00Z
    data/backup.zip (890.5 MB) - 2024-01-15T11:30:00Z
    ... and 40 more objects...
```

### Node Management (Placeholder)

```bash
# Show local node status
cyxcloud node status

# Start local node
cyxcloud node start --storage-gb 100

# Stop local node
cyxcloud node stop
```

---

## Docker Deployment

Docker provides the easiest way to run the full CyxCloud stack with all dependencies.

### Install Docker

**Windows:**
1. Download [Docker Desktop](https://www.docker.com/products/docker-desktop/)
2. Install and restart
3. Enable WSL 2 backend (recommended)

**macOS:**
```bash
# Using Homebrew
brew install --cask docker

# Or download Docker Desktop from docker.com
```

**Linux (Ubuntu/Debian):**
```bash
sudo apt-get update
sudo apt-get install docker.io docker-compose-plugin
sudo systemctl enable --now docker
sudo usermod -aG docker $USER
# Logout and login again
```

**Linux (Arch):**
```bash
sudo pacman -S docker docker-compose
sudo systemctl enable --now docker
sudo usermod -aG docker $USER
newgrp docker
```

**Linux (Fedora):**
```bash
sudo dnf install docker docker-compose-plugin
sudo systemctl enable --now docker
sudo usermod -aG docker $USER
```

### Quick Start

```bash
# Clone repository
git clone https://github.com/CYXWIZ-Lab/cyxcloud.git
cd cyxcloud

# Start the full cluster (PostgreSQL, Redis, Gateway, 3 Nodes, Rebalancer)
docker compose up -d --build

# Check status
docker compose ps

# View logs (all services)
docker compose logs -f

# View specific service logs
docker compose logs -f gateway
docker compose logs -f node1

# Stop cluster
docker compose down

# Stop and remove all data
docker compose down -v
```

### Service Ports

| Service | Container Port | Host Port | Description |
|---------|----------------|-----------|-------------|
| PostgreSQL | 5432 | 5432 | Metadata database |
| Redis | 6379 | 6379 | Cache layer |
| Gateway | 8080, 50052 | 8080, 50052 | S3 API + gRPC |
| Node 1 | 50051, 4001, 9090 | 50061, 4001, 9091 | gRPC, P2P, Metrics |
| Node 2 | 50051, 4001, 9090 | 50062, 4002, 9092 | gRPC, P2P, Metrics |
| Node 3 | 50051, 4001, 9090 | 50063, 4003, 9093 | gRPC, P2P, Metrics |

### Testing the Cluster

```bash
# Check gateway health
curl http://localhost:8080/health

# Create a test file
echo "Hello from CyxCloud!" > testfile.txt

# Upload using curl
curl -X PUT http://localhost:8080/s3/mybucket/testfile.txt \
    -H "Content-Type: text/plain" \
    --data-binary @testfile.txt

# List bucket contents
curl "http://localhost:8080/s3/mybucket?list-type=2"

# Download file
curl http://localhost:8080/s3/mybucket/testfile.txt -o downloaded.txt
cat downloaded.txt
```

### Using CLI with Docker Cluster

```bash
# Download CLI binary for your platform (or build from source)
# Then connect to the Docker cluster:

# Upload file
./cyxcloud --gateway http://localhost:8080 upload testfile.txt -b mybucket

# List files
./cyxcloud --gateway http://localhost:8080 list mybucket

# Download file
./cyxcloud --gateway http://localhost:8080 download mybucket -k testfile.txt -o downloaded.txt

# Check status
./cyxcloud --gateway http://localhost:8080 status
```

### Development Mode

Development mode enables verbose logging and optional management UIs.

```bash
# Start with debug logging
docker compose up -d

# View detailed logs
docker compose logs -f gateway 2>&1 | grep -E "(INFO|DEBUG|WARN|ERROR)"
```

**Environment variables for debugging:**
```yaml
environment:
  RUST_LOG: debug,cyxcloud_gateway=trace
```

### Monitoring

Access Prometheus metrics from each node:

```bash
# Node 1 metrics
curl http://localhost:9091/metrics

# Node 2 metrics
curl http://localhost:9092/metrics

# Node 3 metrics
curl http://localhost:9093/metrics
```

**Key metrics:**
- `cyxcloud_storage_bytes_used` - Storage consumption
- `cyxcloud_chunks_total` - Number of chunks stored
- `cyxcloud_bandwidth_in_bytes` - Incoming bandwidth
- `cyxcloud_bandwidth_out_bytes` - Outgoing bandwidth
- `cyxcloud_heartbeat_success_total` - Successful heartbeats

### Building Docker Images

```bash
# Build all images
docker compose build

# Build specific image
docker build -f Dockerfile.gateway -t cyxcloud-gateway .
docker build -f Dockerfile.node -t cyxcloud-node .
docker build -f Dockerfile.cli -t cyxcloud-cli .

# Build with no cache (clean build)
docker compose build --no-cache
```

### Running CLI in Docker

```bash
# Run CLI commands against the cluster
docker run --rm --network cyxcloud_cyxcloud-net cyxcloud-cli \
    --gateway http://gateway:8080 upload /data/myfile.txt -b mybucket

# Interactive CLI with mounted volume
docker run -it --rm --network cyxcloud_cyxcloud-net \
    -v $(pwd)/data:/workspace/data \
    cyxcloud-cli --gateway http://gateway:8080 list mybucket
```

### Scaling Nodes

Add more storage nodes by modifying `docker-compose.yml`:

```yaml
  node4:
    build:
      context: .
      dockerfile: Dockerfile.node
    container_name: cyxcloud-node4
    ports:
      - "50064:50051"
      - "4004:4001"
      - "9094:9090"
    environment:
      RUST_LOG: info
      NODE_ID: node-4
      NODE_NAME: storage-node-4
      CENTRAL_SERVER_ADDR: http://gateway:50052
      BOOTSTRAP_PEERS: /dns4/node1/tcp/4001,/dns4/node2/tcp/4001,/dns4/node3/tcp/4001
    volumes:
      - node4-data:/data
    networks:
      - cyxcloud-net
    depends_on:
      gateway:
        condition: service_healthy

volumes:
  node4-data:
```

Then restart:
```bash
docker compose up -d
```

### Troubleshooting Docker

**Container won't start:**
```bash
# Check logs
docker compose logs gateway

# Check container status
docker compose ps -a

# Restart specific service
docker compose restart gateway
```

**Port already in use:**
```bash
# Find process using port
lsof -i :8080  # Linux/macOS
netstat -ano | findstr :8080  # Windows

# Change port in docker-compose.yml
ports:
  - "8081:8080"  # Use 8081 on host instead
```

**Database connection issues:**
```bash
# Check if postgres is healthy
docker compose exec postgres pg_isready -U cyxcloud

# View postgres logs
docker compose logs postgres

# Connect to postgres manually
docker compose exec postgres psql -U cyxcloud -d cyxcloud
```

**Reset everything:**
```bash
# Stop all containers and remove volumes
docker compose down -v

# Remove all images
docker compose down --rmi all

# Fresh start
docker compose up -d --build
```

---

## API Reference

### S3-Compatible REST API

The gateway implements a subset of the AWS S3 API.

#### Create Bucket

```bash
curl -X PUT http://localhost:8080/s3/mybucket
```

#### Upload Object

```bash
curl -X PUT http://localhost:8080/s3/mybucket/myfile.txt \
    -H "Content-Type: text/plain" \
    --data-binary @myfile.txt
```

#### Download Object

```bash
curl http://localhost:8080/s3/mybucket/myfile.txt -o myfile.txt

# Partial download (range request)
curl http://localhost:8080/s3/mybucket/largefile.bin \
    -H "Range: bytes=0-999" -o partial.bin
```

#### Head Object (Metadata)

```bash
curl -I http://localhost:8080/s3/mybucket/myfile.txt
```

#### Delete Object

```bash
curl -X DELETE http://localhost:8080/s3/mybucket/myfile.txt
```

#### List Objects

```bash
# List all objects
curl "http://localhost:8080/s3/mybucket?list-type=2"

# List with prefix
curl "http://localhost:8080/s3/mybucket?list-type=2&prefix=data/"

# Limit results
curl "http://localhost:8080/s3/mybucket?list-type=2&max-keys=100"
```

#### Delete Bucket

```bash
curl -X DELETE http://localhost:8080/s3/mybucket
```

### Health Check

```bash
curl http://localhost:8080/health
```

### WebSocket Events (Coming Soon)

```javascript
const ws = new WebSocket('ws://localhost:8080/ws');

ws.onmessage = (event) => {
    const data = JSON.parse(event.data);
    console.log('Event:', data);
};

// Subscribe to topics
ws.send(JSON.stringify({
    type: 'subscribe',
    topics: ['file.*', 'node.*']
}));
```

---

## Configuration

### Environment Variables Summary

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (trace/debug/info/warn/error) |
| `GATEWAY_HOST` | `0.0.0.0` | Gateway bind address |
| `GATEWAY_PORT` | `8080` | Gateway HTTP port |
| `NODE_ID` | `node-1` | Unique node identifier |
| `GRPC_HOST` | `0.0.0.0` | Node gRPC bind address |
| `GRPC_PORT` | `50051` | Node gRPC port |
| `LIBP2P_PORT` | `4001` | Node peer discovery port |
| `STORAGE_PATH` | `/data/chunks` | Chunk storage directory |
| `STORAGE_CAPACITY_GB` | `100` | Storage allocation in GB |
| `BOOTSTRAP_PEERS` | - | Comma-separated peer addresses |
| `SCAN_INTERVAL_SECS` | `3600` | Rebalancer scan interval |
| `REPAIR_PARALLELISM` | `4` | Concurrent repair tasks |

### Node Configuration File (Optional)

Create `config/node.toml`:

```toml
[node]
id = "node-1"
storage_path = "/data/chunks"
capacity_gb = 100

[grpc]
host = "0.0.0.0"
port = 50051

[libp2p]
port = 4001
bootstrap_peers = [
    "/ip4/192.168.1.10/tcp/4001",
    "/ip4/192.168.1.11/tcp/4001"
]

[logging]
level = "info"
```

---

## Troubleshooting

### Gateway Won't Start

```bash
# Check if port is in use
netstat -an | grep 8080

# Try different port
GATEWAY_PORT=8081 cargo run -p cyxcloud-gateway
```

### Nodes Can't Discover Each Other

```bash
# Check libp2p ports are open
netstat -an | grep 4001

# Ensure bootstrap peers are configured
BOOTSTRAP_PEERS="/ip4/192.168.1.10/tcp/4001" cargo run -p cyxcloud-node
```

### Upload Fails

```bash
# Check gateway health
curl http://localhost:8080/health

# Enable debug logging
RUST_LOG=debug cargo run -p cyxcloud-cli -- upload ./file.txt -b test
```

### Docker Container Crashes

```bash
# View container logs
docker-compose logs -f gateway

# Check container status
docker-compose ps

# Restart specific service
docker-compose restart node1
```

### Out of Storage Space

```bash
# Check disk usage
df -h

# Increase node capacity
STORAGE_CAPACITY_GB=200 cargo run -p cyxcloud-node

# Or modify docker-compose.yml and restart
```

### Database Connection Issues

```bash
# Check PostgreSQL is running
docker-compose ps postgres

# Test connection
docker-compose exec postgres psql -U cyxcloud -d cyxcloud -c "SELECT 1"

# View PostgreSQL logs
docker-compose logs postgres
```

---

## Performance Tuning

### Erasure Coding

Default configuration (8 data + 6 parity shards) provides good balance.

For higher redundancy:
```rust
ErasureConfig::new(8, 8)  // 50% overhead, tolerates 8 failures
```

For better storage efficiency:
```rust
ErasureConfig::new(10, 4) // 40% overhead, tolerates 4 failures
```

### Chunk Size

Default: 64 MB chunks. Adjust based on workload:

- **Large files**: 128 MB or 256 MB chunks
- **Small files**: 16 MB or 32 MB chunks
- **Streaming**: 4 MB or 8 MB chunks

### Network Tuning

For high-throughput networks:
```bash
# Increase connection pool
export GRPC_CONNECTION_POOL_SIZE=10

# Increase concurrent streams
export GRPC_MAX_CONCURRENT_STREAMS=100
```

---

## Next Steps

- [Phase 5 Integration](../docs/cyxwiz_integration.md) - Connect to CyxWiz Engine
- [Blockchain Integration](../docs/blockchain.md) - Solana payment setup
- [Production Deployment](../docs/production.md) - Kubernetes manifests
