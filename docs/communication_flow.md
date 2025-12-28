# CyxCloud Communication Flow

Comprehensive documentation of all system communication in the CyxCloud distributed storage platform.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Port Reference](#2-port-reference)
3. [Gateway Component](#3-gateway-component)
4. [Storage Node Component](#4-storage-node-component)
5. [CLI Component](#5-cli-component)
6. [CyxWiz API Integration](#6-cyxwiz-api-integration)
7. [Rebalancer Service](#7-rebalancer-service)
8. [Authentication System](#8-authentication-system)
9. [Communication Flows](#9-communication-flows)
10. [Core Algorithms](#10-core-algorithms)
11. [Database Schema](#11-database-schema)
12. [Storage Reservation System](#12-storage-reservation-system)
13. [Blockchain Integration](#13-blockchain-integration)
14. [Docker Deployment](#14-docker-deployment)
15. [Security Considerations](#15-security-considerations)

---

## 1. System Overview

### 1.1 Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                           CyxCloud Distributed Storage Platform                      │
└─────────────────────────────────────────────────────────────────────────────────────┘

                                    ┌──────────────────┐
                                    │   Solana RPC     │
                                    │   (Blockchain)   │
                                    │   Port: 443      │
                                    └────────┬─────────┘
                                             │ HTTPS
                                             │
┌─────────────────┐                 ┌────────┴─────────┐                 ┌─────────────────┐
│    Website      │ ◄───────────── │                  │ ───────────────►│   PostgreSQL    │
│  (Frontend)     │     HTTP/WS    │     GATEWAY      │      TCP        │   Port: 5432    │
│                 │     8080       │                  │                 │   (Metadata)    │
└─────────────────┘                │   HTTP: 8080     │                 └─────────────────┘
                                   │   gRPC: 50052    │
┌─────────────────┐                │                  │                 ┌─────────────────┐
│      CLI        │ ◄───────────── │                  │ ───────────────►│     Redis       │
│  (cyxcloud-cli) │     HTTP       └────────┬─────────┘      TCP        │   Port: 6379    │
│                 │     8080                │                            │   (Cache)       │
└─────────────────┘                         │ gRPC (50052)              └─────────────────┘
                                            │
                    ┌───────────────────────┼───────────────────────┐
                    │                       │                       │
                    ▼                       ▼                       ▼
           ┌────────────────┐      ┌────────────────┐      ┌────────────────┐
           │  Storage Node  │      │  Storage Node  │      │  Storage Node  │
           │    (node-1)    │◄────►│    (node-2)    │◄────►│    (node-3)    │
           │                │      │                │      │                │
           │  gRPC:  50051  │      │  gRPC:  50051  │      │  gRPC:  50051  │
           │  libp2p: 4001  │      │  libp2p: 4001  │      │  libp2p: 4001  │
           │  metrics: 9090 │      │  metrics: 9090 │      │  metrics: 9090 │
           └────────────────┘      └────────────────┘      └────────────────┘
                    │                       │                       │
                    └───────────────────────┼───────────────────────┘
                                            │ libp2p (P2P)
                                            ▼
                               ┌─────────────────────────┐
                               │   P2P Discovery Mesh    │
                               │   (Future: DHT-based)   │
                               └─────────────────────────┘

External Services:
┌─────────────────┐
│   CyxWiz API    │  ◄── Node authentication, user profiles
│   Port: 3002    │
└─────────────────┘

┌─────────────────┐
│   Rebalancer    │  ◄── Background chunk repair service
│   (Background)  │
└─────────────────┘
```

### 1.2 Component Summary

| Component | Description | Technology Stack |
|-----------|-------------|------------------|
| **Gateway** | Central coordinator, S3-compatible API | Rust, Axum, Tonic gRPC |
| **Storage Node** | Distributed storage worker | Rust, RocksDB, gRPC |
| **CLI** | Command-line interface | Rust, Clap, Reqwest |
| **Rebalancer** | Background chunk repair | Rust, Tokio |
| **CyxWiz API** | Authentication service | Rust, MongoDB |
| **PostgreSQL** | Metadata database | PostgreSQL 15 |
| **Redis** | Caching layer | Redis 7 |

### 1.3 Technology Stack

- **Language**: Rust (100%)
- **HTTP Framework**: Axum (async web framework)
- **gRPC Framework**: Tonic (Rust gRPC implementation)
- **Database**: PostgreSQL 15 (metadata), RocksDB (local node storage)
- **Cache**: Redis 7 (sessions, hot data)
- **Serialization**: Protocol Buffers (gRPC), JSON (REST API)
- **Hashing**: Blake3 (content-addressable storage)
- **Encryption**: AES-256-GCM (optional at-rest encryption)
- **Erasure Coding**: Reed-Solomon (10 data + 4 parity shards)

---

## 2. Port Reference

### 2.1 Complete Port Table

| Component | Port | Protocol | Direction | Purpose |
|-----------|------|----------|-----------|---------|
| **Gateway HTTP** | 8080 | HTTP/HTTPS | Inbound | S3 API, Auth API, WebSocket, Health |
| **Gateway gRPC** | 50052 | gRPC | Inbound | NodeService, DataService |
| **Storage Node gRPC** | 50051 | gRPC | Inbound | ChunkService (store/retrieve) |
| **Storage Node libp2p** | 4001 | libp2p/TCP | Bidirectional | P2P discovery, DHT (future) |
| **Storage Node Metrics** | 9090 | HTTP | Inbound | Prometheus metrics, health |
| **CyxWiz API** | 3002 | HTTP | Inbound | User auth, machine registry |
| **PostgreSQL** | 5432 | TCP | Internal | Metadata storage |
| **Redis** | 6379 | TCP | Internal | Caching, sessions |
| **Solana RPC** | 443 | HTTPS | Outbound | Blockchain operations |

### 2.2 Docker Port Mappings

```yaml
# External → Internal port mappings
Gateway:     8080:8080, 50052:50052
Node 1:      50061:50051, 4001:4001, 9091:9090
Node 2:      50062:50051, 4002:4001, 9092:9090
Node 3:      50063:50051, 4003:4001, 9093:9090
PostgreSQL:  5432:5432
Redis:       6379:6379
CyxWiz API:  3002:3002
```

### 2.3 Network Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    Docker Network (cyxcloud-net)                 │
│                       Subnet: 172.28.0.0/16                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
│  │  postgres   │  │    redis    │  │ cyxwiz-api  │              │
│  │    :5432    │  │    :6379    │  │    :3002    │              │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘              │
│         │                │                │                      │
│         └────────────────┼────────────────┘                      │
│                          │                                       │
│                    ┌─────┴─────┐                                 │
│                    │  gateway  │                                 │
│                    │ :8080     │ ◄── HTTP/S3/WS                  │
│                    │ :50052    │ ◄── gRPC                        │
│                    └─────┬─────┘                                 │
│                          │                                       │
│         ┌────────────────┼────────────────┐                      │
│         │                │                │                      │
│    ┌────┴────┐     ┌─────┴────┐     ┌────┴────┐                  │
│    │  node1  │     │  node2   │     │  node3  │                  │
│    │ :50051  │◄───►│  :50051  │◄───►│ :50051  │                  │
│    │ :4001   │     │  :4001   │     │ :4001   │                  │
│    │ :9090   │     │  :9090   │     │ :9090   │                  │
│    └─────────┘     └──────────┘     └─────────┘                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ Exposed to Host
                              ▼
            ┌─────────────────────────────────┐
            │  Host Machine (localhost)        │
            │                                  │
            │  :8080    → Gateway HTTP         │
            │  :50052   → Gateway gRPC         │
            │  :50061-3 → Node gRPC            │
            │  :4001-3  → Node libp2p          │
            │  :9091-3  → Node metrics         │
            │  :5432    → PostgreSQL           │
            │  :6379    → Redis                │
            │  :3002    → CyxWiz API           │
            └─────────────────────────────────┘
```

---

## 3. Gateway Component

### 3.1 Overview

The Gateway is the central coordinator of the CyxCloud network. It provides:

- **S3-compatible REST API** for file operations
- **gRPC services** for storage node communication
- **WebSocket** for real-time events
- **Authentication** via JWT tokens and wallet signatures

### 3.2 Source Files

| File | Purpose |
|------|---------|
| `cyxcloud-gateway/src/main.rs` | Entry point, CLI args, server setup |
| `cyxcloud-gateway/src/state.rs` | Application state, chunk management |
| `cyxcloud-gateway/src/s3_api.rs` | S3-compatible API routes |
| `cyxcloud-gateway/src/auth.rs` | JWT token generation/validation |
| `cyxcloud-gateway/src/auth_api.rs` | Authentication REST endpoints |
| `cyxcloud-gateway/src/grpc_api.rs` | gRPC service implementations |
| `cyxcloud-gateway/src/websocket.rs` | WebSocket real-time events |
| `cyxcloud-gateway/src/node_client.rs` | gRPC client for storage nodes |
| `cyxcloud-gateway/src/node_monitor.rs` | Node lifecycle monitoring |

### 3.3 HTTP Routes

```
GET  /health              → Health check ("OK")
GET  /version             → Version string

# Authentication API (/api/v1/auth)
POST /api/v1/auth/challenge         → Get signing challenge
POST /api/v1/auth/wallet            → Wallet-based login
POST /api/v1/auth/refresh           → Refresh access token
POST /api/v1/auth/api-keys          → Create API key
GET  /api/v1/auth/api-keys          → List API keys
DELETE /api/v1/auth/api-keys/:id    → Revoke API key
POST /api/v1/auth/logout            → Logout (blacklist token)
GET  /api/v1/auth/me                → Get current user info

# S3-Compatible API (/s3)
GET    /s3/:bucket                  → List objects in bucket
PUT    /s3/:bucket                  → Create bucket
DELETE /s3/:bucket                  → Delete bucket
GET    /s3/:bucket/*key             → Download object
PUT    /s3/:bucket/*key             → Upload object
HEAD   /s3/:bucket/*key             → Get object metadata
DELETE /s3/:bucket/*key             → Delete object

# WebSocket
GET /ws                             → WebSocket connection
```

### 3.4 gRPC Services

```protobuf
// NodeService - Storage node management
service NodeService {
    rpc RegisterNode(RegisterNodeRequest) returns (RegisterNodeResponse);
    rpc Heartbeat(HeartbeatRequest) returns (HeartbeatResponse);
    rpc GetNodeInfo(GetNodeInfoRequest) returns (NodeInfo);
    rpc ListNodes(ListNodesRequest) returns (ListNodesResponse);
}

// DataService - Data streaming for ML training
service DataService {
    rpc StreamData(StreamDataRequest) returns (stream DataChunk);
    rpc GetDatasetInfo(GetDatasetInfoRequest) returns (DatasetInfo);
}
```

### 3.5 CLI Arguments

```bash
cyxcloud-gateway [OPTIONS]

OPTIONS:
    --http-addr <ADDR>       HTTP listen address [default: 0.0.0.0:8180]
    --grpc-addr <ADDR>       gRPC listen address [default: 0.0.0.0:50052]
    --cors-permissive        Enable CORS for all origins
    --database-url <URL>     PostgreSQL connection URL
    --redis-url <URL>        Redis connection URL
    --memory-only            Force in-memory storage
    --grpc-auth              Enable gRPC authentication
    --solana-rpc-url <URL>   Solana RPC endpoint (blockchain feature)
    --keypair-path <PATH>    Gateway authority keypair
    --enable-blockchain      Enable blockchain integration
```

### 3.6 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection string | None (memory mode) |
| `REDIS_URL` | Redis connection string | None |
| `JWT_SECRET` | Secret for JWT signing | Required |
| `RUST_LOG` | Logging level | `info` |
| `SOLANA_RPC_URL` | Solana RPC endpoint | `https://api.devnet.solana.com` |

### 3.7 Outbound Connections

| Destination | Port | Protocol | Purpose |
|-------------|------|----------|---------|
| PostgreSQL | 5432 | TCP | Metadata queries/updates |
| Redis | 6379 | TCP | Caching, session storage |
| Storage Nodes | 50051 | gRPC | Chunk store/retrieve |
| Solana RPC | 443 | HTTPS | Blockchain operations |

---

## 4. Storage Node Component

### 4.1 Overview

Storage Nodes are the distributed workers that store and retrieve data chunks. Each node:

- Registers with the Gateway on startup
- Sends periodic heartbeats (every 30 seconds)
- Stores chunks in local RocksDB database
- Participates in P2P discovery via libp2p

### 4.2 Source Files

| File | Purpose |
|------|---------|
| `cyxcloud-node/src/main.rs` | Entry point, startup sequence |
| `cyxcloud-node/src/lib.rs` | Node configuration, startup |
| `cyxcloud-node/src/config.rs` | Configuration loading |
| `cyxcloud-node/src/machine_service.rs` | gRPC ChunkService implementation |
| `cyxcloud-node/src/health.rs` | Health check endpoint |
| `cyxcloud-node/src/cyxwiz_api_client.rs` | CyxWiz API authentication |

### 4.3 gRPC Services (Provided by Node)

```protobuf
// ChunkService - Chunk storage operations
service ChunkService {
    rpc StoreChunk(StoreChunkRequest) returns (StoreChunkResponse);
    rpc GetChunk(GetChunkRequest) returns (GetChunkResponse);
    rpc DeleteChunk(DeleteChunkRequest) returns (DeleteChunkResponse);
    rpc HasChunk(HasChunkRequest) returns (HasChunkResponse);
    rpc ListChunks(ListChunksRequest) returns (ListChunksResponse);
}

// DataService - Chunk replication
service DataService {
    rpc ReplicateChunk(ReplicateChunkRequest) returns (ReplicateChunkResponse);
    rpc VerifyChunk(VerifyChunkRequest) returns (VerifyChunkResponse);
}
```

### 4.4 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `NODE_ID` | Unique node identifier | Generated UUID |
| `NODE_NAME` | Human-readable name | `storage-node` |
| `GRPC_HOST` | gRPC bind address | `0.0.0.0` |
| `GRPC_PORT` | gRPC listen port | `50051` |
| `LIBP2P_PORT` | libp2p listen port | `4001` |
| `METRICS_PORT` | Prometheus metrics port | `9090` |
| `STORAGE_PATH` | Local storage directory | `/data/storage` |
| `STORAGE_CAPACITY_GB` | Advertised capacity | `100` |
| `CENTRAL_SERVER_ADDR` | Gateway gRPC address | Required |
| `BOOTSTRAP_PEERS` | libp2p bootstrap peers | Empty |
| `CYXWIZ_API_URL` | CyxWiz API endpoint | Required |
| `CYXWIZ_EMAIL` | Auth email | Required |
| `CYXWIZ_PASSWORD` | Auth password | Required |

### 4.5 Startup Sequence

```
1. Load configuration (env vars, config file)
2. Authenticate with CyxWiz API (email/password → JWT)
3. Initialize RocksDB storage
4. Start gRPC server (port 50051)
5. Start health check endpoint (port 9090)
6. Register with Gateway (RegisterNode RPC)
7. Start heartbeat loop (every 30 seconds)
8. Start libp2p discovery (optional)
```

### 4.6 Heartbeat Mechanism

```
┌──────────────┐                    ┌──────────────┐
│ Storage Node │                    │   Gateway    │
└──────┬───────┘                    └──────┬───────┘
       │                                   │
       │  Heartbeat (every 30s)            │
       ├──────────────────────────────────►│
       │  - node_id                        │
       │  - storage_used                   │
       │  - storage_available              │
       │  - cpu_usage                      │
       │  - memory_usage                   │
       │  - active_connections             │
       │                                   │
       │◄──────────────────────────────────┤
       │  HeartbeatResponse                │
       │  - commands (optional)            │
       │                                   │
```

### 4.7 Node Status Lifecycle

```
                    ┌──────────┐
                    │  offline │
                    └────┬─────┘
                         │ RegisterNode
                         ▼
                    ┌──────────┐
         ┌─────────│  online  │◄────────────┐
         │         └────┬─────┘             │
         │              │                    │
         │  No heartbeat│                    │ Heartbeat OK
         │  (1 minute)  │                    │ (5 min quarantine)
         ▼              ▼                    │
    ┌──────────┐   ┌──────────┐             │
    │ offline  │   │recovering│─────────────┘
    └────┬─────┘   └──────────┘
         │
         │ No heartbeat (4 hours)
         ▼
    ┌──────────┐
    │ draining │ ◄── Chunks being migrated
    └────┬─────┘
         │
         │ No heartbeat (7 days)
         ▼
    ┌──────────┐
    │ (removed)│ ◄── Node deleted from database
    └──────────┘
```

### 4.6 Node Lifecycle Timers

The Gateway uses several configurable timers to manage node health:

| Timer | Default | Description |
|-------|---------|-------------|
| Heartbeat Interval | 30 seconds | How often nodes send heartbeats |
| Offline Threshold | 5 minutes | Time without heartbeat before marking offline |
| Recovery Quarantine | 5 minutes | Time node must stay healthy before going back online |
| Drain Threshold | 4 hours | Time offline before starting chunk migration |
| Remove Threshold | 7 days | Time offline before removing node from database |

#### Recovery Quarantine Explained

When a node comes back online after being offline, it enters a **5-minute quarantine period** (status: `recovering`):

```
Timeline Example:
═══════════════════════════════════════════════════════════════════════

00:00  Node goes offline (gateway down, network issue, etc.)
       └─ Status: offline

00:05  Node sends heartbeat, Gateway receives it
       └─ Status: recovering (NOT online yet!)
       └─ Node cannot receive new chunk storage requests

00:10  Node still sending heartbeats every 30 seconds
       └─ Status: still recovering (quarantine not passed)

00:10  Quarantine period (5 min) passes with stable heartbeats
       └─ Status: online ✓
       └─ Node can now receive chunk storage requests
```

**Why Quarantine?**

The quarantine prevents "flapping" nodes from immediately receiving data:
- A node that keeps going online/offline is unreliable
- Data stored on flapping nodes risks being lost
- Quarantine ensures node stability before trusting it with data

#### Configuring Timers (Gateway)

Timers are configured via environment variables in the Gateway:

```bash
# docker-compose.yml
environment:
  NODE_OFFLINE_THRESHOLD_MINS: 5        # Default: 5
  NODE_RECOVERY_QUARANTINE_MINS: 5      # Default: 5
  NODE_DRAIN_THRESHOLD_HOURS: 4         # Default: 4
  NODE_REMOVE_THRESHOLD_DAYS: 7         # Default: 7
```

#### Storage Reservation

When a node registers, the Gateway reserves **2 GB** of the node's storage for system use:

```
Storage Breakdown (100 GB node example):
┌─────────────────────────────────────────────────────────────────┐
│                     TOTAL STORAGE (100 GB)                      │
├──────────────┬──────────────────────────────────────────────────┤
│   RESERVED   │              ALLOCATABLE (98 GB)                 │
│    (2 GB)    ├──────────────────────┬───────────────────────────┤
│              │        USED          │       AVAILABLE           │
│  System use: │   (stored chunks)    │   (free for new chunks)   │
│  - Metadata  │                      │                           │
│  - Parity    │                      │                           │
│  - Rebalance │                      │                           │
└──────────────┴──────────────────────┴───────────────────────────┘
```

The node displays this on successful registration:
```
========================================
  Storage Reservation Summary
========================================
  Total Capacity:  100.0 GB
  System Reserved: 2.0 GB
  Available:       98.0 GB
========================================
```

### 4.7 Node Operator Terms & Conditions

This section explains the rules and penalties for running a CyxCloud storage node.

#### Uptime Requirements

| Requirement | Threshold | Consequence |
|-------------|-----------|-------------|
| Heartbeat | Every 30 seconds | Required to maintain "online" status |
| Maximum Downtime | 5 minutes | Node marked "offline", stops receiving new chunks |
| Extended Downtime | 4 hours | Node enters "draining" mode, chunks migrated away |
| Abandonment | 7 days | Node removed from network permanently |

#### Downtime Penalties

```
Downtime Timeline & Consequences
═══════════════════════════════════════════════════════════════════════

 0-5 min     │ Grace period - no penalty
             │ Node still "online"
─────────────┼─────────────────────────────────────────────────────────
 5 min       │ Node marked "OFFLINE"
             │ • No new chunks assigned
             │ • Existing data still served if node comes back
             │ • No earnings during offline period
─────────────┼─────────────────────────────────────────────────────────
 4 hours     │ Node enters "DRAINING" mode
             │ • Chunks actively migrated to other nodes
             │ • Network prepares for permanent loss
             │ • Reputation score decreased
─────────────┼─────────────────────────────────────────────────────────
 7 days      │ Node "REMOVED" from network
             │ • All chunk assignments transferred
             │ • Must re-register as new node
             │ • Staked tokens enter unstake cooldown
═══════════════════════════════════════════════════════════════════════
```

#### Staking & Slashing (Blockchain-Enabled)

When blockchain integration is enabled, node operators must stake tokens:

| Parameter | Value | Description |
|-----------|-------|-------------|
| Minimum Stake | 500 CYXWIZ | Required to operate a node |
| Unstake Lockup | 7 days | Cooldown period to withdraw stake |

**Slashing Penalties** (% of staked tokens):

| Violation | Penalty | Description |
|-----------|---------|-------------|
| Extended Downtime | 5% | Offline > 24 hours without notice |
| Data Loss | 10% | Unable to serve chunks you're responsible for |
| Failed Proofs | 15% | Failed 3+ consecutive proof-of-storage challenges |
| Corrupted Data | 50% | Serving invalid/corrupted data to users |

#### Reputation System

Nodes earn reputation based on performance:

```
Reputation Score (0-10,000 points)
═══════════════════════════════════════════════════════════════════════

Positive Factors:                    Negative Factors:
─────────────────────────────────    ─────────────────────────────────
+10  Successful proof-of-storage     -50  Failed proof-of-storage
+5   Fast chunk retrieval (<100ms)   -100 Downtime event
+1   Each hour of uptime             -500 Data loss event
+20  Perfect weekly uptime           -1000 Slashing event

Benefits of High Reputation:
• Priority for chunk assignments (more storage = more earnings)
• Higher trust score shown to users
• Eligible for premium storage tiers
• Lower proof-of-storage challenge frequency
═══════════════════════════════════════════════════════════════════════
```

#### Earnings & Payments

| Parameter | Value | Description |
|-----------|-------|-------------|
| Payment Epoch | Weekly | Earnings distributed every 7 days |
| Node Share | 85% | Of storage fees for chunks you store |
| Platform Fee | 10% | Goes to CyxCloud platform |
| Community Fund | 5% | For network development |

**Earnings Calculation:**
```
Weekly Earnings = (Chunks Stored × Chunk Size × Price per GB/month × 7/30) × 0.85
```

#### Best Practices for Node Operators

1. **Stable Internet**: Use wired connection, avoid shared/congested networks
2. **UPS/Battery Backup**: Prevent data loss during power outages
3. **Monitoring**: Set up alerts for disk space, CPU, and connectivity
4. **Planned Maintenance**: Use `--maintenance` flag to gracefully pause
5. **Sufficient Storage**: Leave 20% headroom beyond your committed capacity
6. **Regular Updates**: Keep node software updated for security patches

#### Quick Reference Card

```
┌─────────────────────────────────────────────────────────────────────┐
│                    NODE OPERATOR QUICK REFERENCE                    │
├─────────────────────────────────────────────────────────────────────┤
│  STAY ONLINE        │  Heartbeat every 30s, max 5min downtime      │
│  STAKE REQUIRED     │  500 CYXWIZ minimum                          │
│  EARNINGS           │  85% of storage fees, paid weekly            │
│  WORST PENALTY      │  50% stake slash for corrupted data          │
│  RECOVERY TIME      │  5 min quarantine after coming back online   │
│  LOGOUT COMMAND     │  cyxcloud-node --logout                      │
│  CHECK STATUS       │  curl http://localhost:9090/health           │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 5. CLI Component

### 5.1 Overview

The CLI (`cyxcloud-cli`) provides command-line access to CyxCloud storage:

- User authentication (wallet-based)
- File upload/download
- Bucket management
- Storage info

### 5.2 Source Files

| File | Purpose |
|------|---------|
| `cyxcloud-cli/src/main.rs` | Entry point, command dispatch |
| `cyxcloud-cli/src/client.rs` | HTTP client for Gateway |
| `cyxcloud-cli/src/config.rs` | Configuration management |
| `cyxcloud-cli/src/cyxwiz_client.rs` | CyxWiz API client |
| `cyxcloud-cli/src/commands/auth.rs` | Login/logout commands |
| `cyxcloud-cli/src/commands/upload.rs` | File upload |
| `cyxcloud-cli/src/commands/download.rs` | File download |
| `cyxcloud-cli/src/commands/delete.rs` | File deletion |

### 5.3 Commands

```bash
cyxcloud-cli [OPTIONS] <COMMAND>

COMMANDS:
    login       Authenticate with CyxCloud
    logout      Clear stored credentials
    upload      Upload file to storage
    download    Download file from storage
    delete      Delete file from storage
    list        List files in bucket
    info        Show storage usage info
    help        Print help information

OPTIONS:
    --gateway <URL>    Gateway URL [default: http://localhost:8080]
    --config <PATH>    Config file path [default: ~/.cyxcloud/config.toml]
    -v, --verbose      Enable verbose output
```

### 5.4 Configuration File

Location: `~/.cyxcloud/config.toml`

```toml
# CyxCloud CLI Configuration

[gateway]
url = "http://localhost:8080"

[auth]
access_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
refresh_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
wallet_address = "5xK4gM..."
expires_at = "2024-12-29T00:00:00Z"

[defaults]
bucket = "default"
```

### 5.5 Authentication Flow (CLI)

```
┌─────────────┐                    ┌─────────────┐
│     CLI     │                    │   Gateway   │
└──────┬──────┘                    └──────┬──────┘
       │                                  │
       │  POST /api/v1/auth/challenge     │
       ├─────────────────────────────────►│
       │                                  │
       │◄─────────────────────────────────┤
       │  { challenge: "...", nonce: ... }│
       │                                  │
       │  [User signs with wallet]        │
       │                                  │
       │  POST /api/v1/auth/wallet        │
       │  { wallet_address, signature }   │
       ├─────────────────────────────────►│
       │                                  │
       │◄─────────────────────────────────┤
       │  { access_token, refresh_token } │
       │                                  │
       │  [Store tokens in config.toml]   │
       │                                  │
```

---

## 6. CyxWiz API Integration

### 6.1 Overview

The CyxWiz API is the authentication and user management service. It provides:

- User registration and login
- Machine (storage node) registration
- Wallet address management
- User profile and subscription data

### 6.2 Connection Details

| Property | Value |
|----------|-------|
| **URL** | `http://cyxwiz-api:3002` (internal) or configured |
| **Protocol** | HTTP REST |
| **Database** | MongoDB |

### 6.3 Key Endpoints

```
# Authentication
POST /api/auth/login          → Email/password login
POST /api/auth/register       → User registration
POST /api/auth/refresh        → Refresh token

# User Profile
GET  /api/users/me            → Get current user
PUT  /api/users/me            → Update user profile

# Machine (Node) Registration
POST /api/machines/register   → Register storage node
GET  /api/machines            → List user's machines
DELETE /api/machines/:id      → Unregister machine

# Wallet
GET  /api/wallets/me          → Get user's wallet info
POST /api/wallets/verify      → Verify wallet ownership
```

### 6.4 Integration Points

**Storage Node → CyxWiz API:**
- Login with email/password on startup
- Register machine with CyxWiz API
- Use returned JWT token for Gateway authentication

**Gateway → CyxWiz API:**
- Shared JWT_SECRET for token validation
- User lookup for storage quota checks

---

## 7. Rebalancer Service

### 7.1 Overview

The Rebalancer is a background service that maintains data durability by:

- Monitoring chunk replication status
- Detecting under-replicated chunks
- Triggering chunk repair/replication
- Ensuring minimum replica count

### 7.2 Source Files

| File | Purpose |
|------|---------|
| `cyxcloud-rebalancer/src/main.rs` | Entry point |
| `cyxcloud-rebalancer/src/lib.rs` | Core rebalancing logic |

### 7.3 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection | Required |
| `REDIS_URL` | Redis connection | Optional |
| `SCAN_INTERVAL_SECS` | Scan interval | `300` (5 min) |
| `REPAIR_PARALLELISM` | Concurrent repairs | `4` |
| `MIN_REPLICAS` | Minimum replica count | `3` |

### 7.4 Rebalancing Algorithm

```
Every SCAN_INTERVAL_SECS:
1. Query chunks WHERE current_replicas < replication_factor
2. For each under-replicated chunk:
   a. Find nodes that have the chunk (source nodes)
   b. Find healthy nodes without the chunk (target nodes)
   c. Select target based on:
      - Available capacity
      - Network distance (future)
      - Rack diversity (future)
   d. Create repair job in database
3. Execute repair jobs (up to REPAIR_PARALLELISM concurrent):
   a. Fetch chunk from source node
   b. Store chunk on target node
   c. Update chunk_locations table
   d. Increment current_replicas count
```

### 7.5 Repair Job States

```
pending → in_progress → completed
              ↓
           failed (retry)
```

---

## 8. Authentication System

### 8.1 Overview

CyxCloud uses a dual-authentication system:

1. **Wallet-based auth** (primary): Solana Ed25519 signatures
2. **JWT tokens**: For session management

### 8.2 Token Types

| Token Type | Lifetime | Purpose | Claim `typ` |
|------------|----------|---------|-------------|
| **Access Token** | 1 hour | API calls | `access` |
| **Refresh Token** | 7 days | Token renewal | `refresh` |
| **Node Token** | 24 hours | Storage node auth | `node` |
| **API Key** | 365 days | Programmatic access | `api_key` |

### 8.3 JWT Claims

```json
{
  "sub": "user-uuid-here",
  "wallet": "5xK4gM...",
  "typ": "access",
  "iat": 1703808000,
  "exp": 1703811600,
  "jti": "unique-token-id"
}
```

### 8.4 Challenge-Response Flow

```
┌──────────────┐                    ┌──────────────┐
│    Client    │                    │   Gateway    │
└──────┬───────┘                    └──────┬───────┘
       │                                   │
       │  1. POST /auth/challenge          │
       │     { wallet_address }            │
       ├──────────────────────────────────►│
       │                                   │
       │◄──────────────────────────────────┤
       │  2. { challenge, nonce, expires } │
       │     "CyxCloud Auth\nWallet: ...\n"│
       │     "Nonce: abc123\nTime: ..."    │
       │                                   │
       │  [Client signs message with       │
       │   wallet private key]             │
       │                                   │
       │  3. POST /auth/wallet             │
       │     { wallet_address, signature,  │
       │       challenge, nonce }          │
       ├──────────────────────────────────►│
       │                                   │
       │  4. Gateway verifies:             │
       │     - Nonce not used before       │
       │     - Challenge not expired       │
       │     - Ed25519 signature valid     │
       │                                   │
       │◄──────────────────────────────────┤
       │  5. { access_token, refresh_token │
       │       expires_in: 3600 }          │
       │                                   │
```

### 8.5 Signature Verification

```rust
// Solana Ed25519 signature verification
fn verify_signature(
    wallet_address: &str,  // Base58 public key
    message: &str,         // Challenge message
    signature: &str,       // Base58 signature
) -> bool {
    let pubkey = Pubkey::from_str(wallet_address)?;
    let sig = Signature::from_str(signature)?;
    sig.verify(pubkey.as_ref(), message.as_bytes())
}
```

### 8.6 Token Refresh

```
POST /api/v1/auth/refresh
Authorization: Bearer <refresh_token>

Response:
{
  "access_token": "new-access-token",
  "expires_in": 3600
}
```

---

## 9. Communication Flows

### 9.1 User Login Flow

```
┌─────────────┐           ┌─────────────┐           ┌─────────────┐
│   Browser   │           │   Gateway   │           │  PostgreSQL │
│   /Website  │           │   (:8080)   │           │   (:5432)   │
└──────┬──────┘           └──────┬──────┘           └──────┬──────┘
       │                         │                         │
       │ 1. GET /auth/challenge  │                         │
       │    {wallet_address}     │                         │
       ├────────────────────────►│                         │
       │                         │                         │
       │◄────────────────────────┤                         │
       │ 2. {challenge, nonce}   │                         │
       │                         │                         │
       │ [Sign with wallet]      │                         │
       │                         │                         │
       │ 3. POST /auth/wallet    │                         │
       │    {signature, ...}     │                         │
       ├────────────────────────►│                         │
       │                         │                         │
       │                         │ 4. Verify signature     │
       │                         │    (Ed25519)            │
       │                         │                         │
       │                         │ 5. Lookup/create user   │
       │                         ├────────────────────────►│
       │                         │◄────────────────────────┤
       │                         │    {user record}        │
       │                         │                         │
       │◄────────────────────────┤                         │
       │ 6. {access_token,       │                         │
       │     refresh_token}      │                         │
       │                         │                         │
```

### 9.2 File Upload Flow

```
┌──────────┐        ┌──────────┐        ┌──────────┐        ┌──────────┐
│   CLI    │        │ Gateway  │        │PostgreSQL│        │  Nodes   │
└────┬─────┘        └────┬─────┘        └────┬─────┘        └────┬─────┘
     │                   │                   │                   │
     │ 1. PUT /s3/bucket/key                 │                   │
     │    [file data]    │                   │                   │
     ├──────────────────►│                   │                   │
     │                   │                   │                   │
     │                   │ 2. Chunk file     │                   │
     │                   │    (4MB chunks)   │                   │
     │                   │                   │                   │
     │                   │ 3. Erasure encode │                   │
     │                   │    (10+4 Reed-    │                   │
     │                   │     Solomon)      │                   │
     │                   │                   │                   │
     │                   │ 4. Select nodes   │                   │
     │                   ├──────────────────►│                   │
     │                   │   (query healthy  │                   │
     │                   │    nodes)         │                   │
     │                   │◄──────────────────┤                   │
     │                   │                   │                   │
     │                   │ 5. StoreChunk (gRPC) to each node     │
     │                   ├──────────────────────────────────────►│
     │                   │◄──────────────────────────────────────┤
     │                   │   (14 shards distributed)             │
     │                   │                   │                   │
     │                   │ 6. Record metadata│                   │
     │                   ├──────────────────►│                   │
     │                   │   (files, chunks, │                   │
     │                   │    chunk_locations│                   │
     │                   │    tables)        │                   │
     │                   │◄──────────────────┤                   │
     │                   │                   │                   │
     │◄──────────────────┤                   │                   │
     │ 7. 200 OK         │                   │                   │
     │    {etag, ...}    │                   │                   │
     │                   │                   │                   │
```

### 9.3 File Download Flow

```
┌──────────┐        ┌──────────┐        ┌──────────┐        ┌──────────┐
│   CLI    │        │ Gateway  │        │PostgreSQL│        │  Nodes   │
└────┬─────┘        └────┬─────┘        └────┬─────┘        └────┬─────┘
     │                   │                   │                   │
     │ 1. GET /s3/bucket/key                 │                   │
     ├──────────────────►│                   │                   │
     │                   │                   │                   │
     │                   │ 2. Get file metadata                  │
     │                   ├──────────────────►│                   │
     │                   │◄──────────────────┤                   │
     │                   │   {file, chunks,  │                   │
     │                   │    locations}     │                   │
     │                   │                   │                   │
     │                   │ 3. GetChunk (gRPC) from nodes         │
     │                   ├──────────────────────────────────────►│
     │                   │   (fetch 10+ shards)                  │
     │                   │◄──────────────────────────────────────┤
     │                   │                   │                   │
     │                   │ 4. Erasure decode │                   │
     │                   │    (any 10 of 14  │                   │
     │                   │     shards)       │                   │
     │                   │                   │                   │
     │                   │ 5. Reassemble     │                   │
     │                   │    chunks         │                   │
     │                   │                   │                   │
     │◄──────────────────┤                   │                   │
     │ 6. [file data]    │                   │                   │
     │                   │                   │                   │
```

### 9.4 Node Registration Flow

```
┌──────────────┐        ┌──────────────┐        ┌──────────────┐
│ Storage Node │        │   Gateway    │        │  PostgreSQL  │
└──────┬───────┘        └──────┬───────┘        └──────┬───────┘
       │                       │                       │
       │ [Startup: auth with   │                       │
       │  CyxWiz API first]    │                       │
       │                       │                       │
       │ 1. RegisterNode (gRPC)│                       │
       │    {peer_id, address, │                       │
       │     capacity, region} │                       │
       ├──────────────────────►│                       │
       │                       │                       │
       │                       │ 2. Health check       │
       │◄──────────────────────┤    (verify node)      │
       │      GET /health      │                       │
       ├──────────────────────►│                       │
       │       "OK"            │                       │
       │                       │                       │
       │                       │ 3. Insert/update node │
       │                       ├──────────────────────►│
       │                       │◄──────────────────────┤
       │                       │                       │
       │◄──────────────────────┤                       │
       │ 4. {success, node_id, │                       │
       │     token (optional)} │                       │
       │                       │                       │
       │ [Start heartbeat loop]│                       │
       │                       │                       │
       │ 5. Heartbeat (30s)    │                       │
       ├──────────────────────►│                       │
       │                       │ 6. Update last_heartbeat
       │                       ├──────────────────────►│
       │                       │◄──────────────────────┤
       │◄──────────────────────┤                       │
       │   {status: ok}        │                       │
       │                       │                       │
```

### 9.5 WebSocket Events Flow

```
┌──────────────┐                    ┌──────────────┐
│   Browser    │                    │   Gateway    │
└──────┬───────┘                    └──────┬───────┘
       │                                   │
       │ 1. WS Connect /ws                 │
       │    ?token=<jwt>&topics=file,node  │
       ├──────────────────────────────────►│
       │                                   │
       │◄══════════════════════════════════╡
       │ 2. Connected (upgrade successful) │
       │                                   │
       │ [... time passes ...]             │
       │                                   │
       │◄──────────────────────────────────┤
       │ 3. Event: FileCreated             │
       │    {type: "file.created",         │
       │     data: {bucket, key, size}}    │
       │                                   │
       │◄──────────────────────────────────┤
       │ 4. Event: NodeJoined              │
       │    {type: "cluster.node_joined",  │
       │     data: {node_id, address}}     │
       │                                   │
       │◄──────────────────────────────────┤
       │ 5. Event: UploadProgress          │
       │    {type: "file.progress",        │
       │     data: {key, progress: 75}}    │
       │                                   │
```

---

## 10. Core Algorithms

### 10.1 Chunking

Files are split into fixed-size chunks before distribution:

```
┌──────────────────────────────────────────────────────────┐
│                     Original File                         │
│                        40 MB                              │
└──────────────────────────────────────────────────────────┘
                            │
                            ▼ Split into 4MB chunks
┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐
│  Chunk 0   │ │  Chunk 1   │ │  Chunk 2   │ │  Chunk 3   │ │  Chunk 4   │
│    4MB     │ │    4MB     │ │    4MB     │ │    4MB     │ │    4MB     │
└────────────┘ └────────────┘ └────────────┘ └────────────┘ └────────────┘
                            │
                            ▼ Last chunk padded if needed
                              (padding trimmed on download)
```

**Configuration:**
- Default chunk size: 4 MB (4,194,304 bytes)
- Configurable per-upload
- Last chunk padded to chunk size

### 10.2 Content-Addressable Storage (Blake3)

Each chunk is identified by its Blake3 hash:

```rust
let chunk_id = blake3::hash(&chunk_data);  // 32 bytes
let chunk_id_hex = chunk_id.to_hex();      // 64 char hex string
```

**Benefits:**
- Deduplication (same content = same ID)
- Integrity verification
- Fast (optimized for modern CPUs)

### 10.3 Erasure Coding (Reed-Solomon 10+4)

Each chunk is encoded into 14 shards (10 data + 4 parity):

```
┌────────────┐
│   Chunk    │
│   (4MB)    │
└─────┬──────┘
      │
      ▼ Reed-Solomon Encode
┌───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┬───┐
│ D0│ D1│ D2│ D3│ D4│ D5│ D6│ D7│ D8│ D9│ P0│ P1│ P2│ P3│
└───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┴───┘
  │                             │   │                   │
  └──────────── Data ───────────┘   └────── Parity ─────┘
         (10 shards)                    (4 shards)

To reconstruct: Need any 10 of 14 shards
Fault tolerance: Can lose up to 4 shards
```

**Implementation:**
```rust
use reed_solomon_erasure::galois_8::ReedSolomon;

let rs = ReedSolomon::new(10, 4)?;  // 10 data, 4 parity
let shards: Vec<Vec<u8>> = split_into_shards(chunk, 10);
rs.encode(&mut shards)?;            // Now 14 shards
```

### 10.4 Node Selection Algorithm

When storing chunks, nodes are selected based on:

```rust
fn select_nodes(
    available_nodes: &[Node],
    shard_count: usize,
    min_replicas: usize,
) -> Vec<Node> {
    // 1. Filter healthy nodes (status = "online")
    let healthy: Vec<_> = available_nodes
        .iter()
        .filter(|n| n.status == "online")
        .collect();

    // 2. Sort by available capacity (descending)
    healthy.sort_by_key(|n| -(n.storage_available as i64));

    // 3. Select top N nodes (N = shard_count)
    // Future: Add rack diversity, network distance
    healthy.into_iter().take(shard_count).collect()
}
```

### 10.5 Encryption (Optional)

Client-side AES-256-GCM encryption:

```
┌────────────────┐    Key derivation     ┌────────────────┐
│  User Password │ ─────────────────────►│  256-bit Key   │
└────────────────┘    (Argon2id)         └───────┬────────┘
                                                 │
┌────────────────┐                               ▼
│    Plaintext   │ ──────────────────────► AES-256-GCM
│     Chunk      │                              │
└────────────────┘                              ▼
                                         ┌────────────────┐
                                         │   Ciphertext   │
                                         │     Chunk      │
                                         └────────────────┘
```

**Notes:**
- Encryption is optional (user choice)
- Key never leaves client
- Gateway only sees encrypted bytes

---

## 11. Database Schema

### 11.1 Entity Relationship Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           PostgreSQL Schema                              │
└─────────────────────────────────────────────────────────────────────────┘

┌──────────────┐         ┌──────────────┐         ┌──────────────┐
│    users     │         │   buckets    │         │    nodes     │
├──────────────┤         ├──────────────┤         ├──────────────┤
│ id (PK)      │◄───────┐│ id (PK)      │    ┌───►│ id (PK)      │
│ wallet_addr  │        ││ name         │    │    │ peer_id      │
│ email        │        ││ owner_id(FK)─┼────┘    │ grpc_address │
│ username     │        │└──────────────┘         │ storage_total│
│ storage_quota│        │                         │ storage_used │
│ storage_used │        │                         │ storage_resv │
│ status       │        │                         │ status       │
│ created_at   │        │                         │ last_heartbeat
└──────────────┘        │                         │ first_offline_at
       │                │                         │ status_changed_at
       │                │                         │ failure_count │
       │                │                         │ region        │
       │                │                         └───────┬───────┘
       │                │                                 │
       │                │                                 │
       ▼                ▼                                 │
┌──────────────┐  owner  ┌──────────────┐                 │
│    files     │◄────────│    files     │                 │
├──────────────┤         ├──────────────┤                 │
│ id (PK)      │         │ id (PK)      │                 │
│ name         │         │ owner_id(FK) │                 │
│ path         │         │ bucket       │                 │
│ content_hash │         │ ...          │                 │
│ size_bytes   │         └──────────────┘                 │
│ chunk_count  │                │                         │
│ data_shards  │                │                         │
│ parity_shards│                │                         │
│ chunk_size   │                │                         │
│ status       │                │                         │
│ content_type │                │                         │
│ metadata     │                ▼                         │
└──────┬───────┘         ┌──────────────┐                 │
       │                 │    chunks    │                 │
       │                 ├──────────────┤                 │
       │                 │ id (PK)      │                 │
       └────────────────►│ chunk_id     │                 │
            file_id(FK)  │ file_id(FK)  │                 │
                         │ shard_index  │                 │
                         │ is_parity    │                 │
                         │ size_bytes   │                 │
                         │ replication_factor             │
                         │ current_replicas               │
                         │ status       │                 │
                         └──────┬───────┘                 │
                                │                         │
                                │                         │
                                ▼                         │
                         ┌──────────────┐                 │
                         │chunk_locations                 │
                         ├──────────────┤                 │
                         │ id (PK)      │                 │
                         │ chunk_id(FK) ├─────────────────┤
                         │ node_id(FK) ─┼─────────────────┘
                         │ status       │
                         │ last_verified│
                         │ verification_failures
                         └──────────────┘

                         ┌──────────────┐
                         │ repair_jobs  │
                         ├──────────────┤
                         │ id (PK)      │
                         │ chunk_id     │
                         │ source_node_id
                         │ target_node_id
                         │ status       │
                         │ priority     │
                         │ retry_count  │
                         └──────────────┘
```

### 11.2 Table Descriptions

| Table | Purpose | Key Fields |
|-------|---------|------------|
| `users` | User accounts | wallet_address, storage_quota |
| `buckets` | S3-compatible buckets | name, owner_id |
| `files` | File metadata | path, content_hash, chunk_count |
| `chunks` | Chunk metadata | chunk_id (Blake3), file_id, shard_index |
| `chunk_locations` | Chunk-to-node mapping | chunk_id, node_id, status |
| `nodes` | Storage nodes | peer_id, grpc_address, status |
| `repair_jobs` | Repair task queue | chunk_id, source/target node |

### 11.3 Views

**chunk_replication_status:**
```sql
SELECT
    chunk_id,
    file_id,
    replication_factor,
    current_replicas,
    replication_factor - current_replicas AS replicas_needed,
    CASE
        WHEN current_replicas >= replication_factor THEN 'healthy'
        WHEN current_replicas > 0 THEN 'under_replicated'
        ELSE 'missing'
    END AS health_status
FROM chunks
WHERE status != 'pending';
```

**node_storage_summary:**
```sql
SELECT
    id,
    peer_id,
    grpc_address,
    storage_total,
    storage_reserved,
    storage_used,
    GREATEST(0, storage_total - storage_reserved - storage_used) AS storage_available,
    utilization_percent,
    chunk_count,
    status,
    last_heartbeat
FROM nodes
LEFT JOIN chunk_locations ON ...
GROUP BY nodes.id;
```

---

## 12. Storage Reservation System

### 12.1 Overview

The storage reservation system ensures each node reserves a portion of storage for system operations:

- **Purpose**: Prevent storage exhaustion
- **Reserved space**: 2 GB per node (default)
- **Uses**: Metadata, parity buffers, rebalancing operations, health checks

### 12.2 Calculation

```
storage_available = storage_total - storage_reserved - storage_used
storage_allocatable = storage_total - storage_reserved  # User-visible capacity
```

### 12.3 Database Column

```sql
ALTER TABLE nodes ADD COLUMN storage_reserved BIGINT NOT NULL DEFAULT 0;

-- Comment
COMMENT ON COLUMN nodes.storage_reserved IS
    'Gateway-reserved storage in bytes (2GB default for system use:
     metadata, parity buffer, rebalancing, health checks)';
```

### 12.4 Updated Node Storage Summary View

```sql
CREATE VIEW node_storage_summary AS
SELECT
    n.id,
    n.peer_id,
    n.grpc_address,
    n.storage_total,
    n.storage_reserved,
    n.storage_used,
    -- Available = Total - Reserved - Used
    GREATEST(0, n.storage_total - n.storage_reserved - n.storage_used) AS storage_available,
    -- User-allocatable capacity (excluding reserved)
    GREATEST(0, n.storage_total - n.storage_reserved) AS storage_allocatable,
    CASE
        WHEN (n.storage_total - n.storage_reserved) > 0
        THEN ROUND((n.storage_used::numeric / (n.storage_total - n.storage_reserved)::numeric) * 100, 2)
        ELSE 0
    END AS utilization_percent,
    COUNT(cl.id) AS chunk_count,
    n.status,
    n.last_heartbeat
FROM nodes n
LEFT JOIN chunk_locations cl ON cl.node_id = n.id AND cl.status = 'stored'
GROUP BY n.id;
```

---

## 13. Blockchain Integration

### 13.1 Overview

CyxCloud uses the Solana blockchain for:

- **Payment processing**: Token transfers for storage payments
- **User verification**: Wallet address validation
- **Future**: Payment escrow, node staking, reputation

### 13.2 Configuration

```rust
// Enable with cargo feature: blockchain

#[cfg(feature = "blockchain")]
struct BlockchainConfig {
    solana_rpc_url: String,  // Default: https://api.devnet.solana.com
    keypair_path: Option<String>,
    enable_blockchain: bool,
}
```

### 13.3 Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `SOLANA_RPC_URL` | Solana RPC endpoint | `https://api.devnet.solana.com` |
| `GATEWAY_KEYPAIR_PATH` | Authority keypair file | None |

### 13.4 Wallet Verification Flow

```
1. User provides Solana wallet address (Base58 public key)
2. Gateway generates challenge message
3. User signs message with wallet private key
4. Gateway verifies Ed25519 signature
5. User authenticated by wallet ownership
```

### 13.5 Future: Payment Escrow

```
[Planned - Not Yet Implemented]

1. User creates storage reservation
2. Payment locked in escrow smart contract
3. Storage nodes store data
4. On successful storage proof:
   - 90% released to storage nodes
   - 10% to platform (CyxCloud)
5. On failure: Refund to user
```

---

## 14. Docker Deployment

### 14.1 Docker Compose Overview

The `docker-compose.yml` defines the complete CyxCloud stack:

```yaml
services:
  cyxwiz-api     # Authentication service
  postgres       # Metadata database
  redis          # Caching layer
  gateway        # Central coordinator
  node1          # Storage node 1
  node2          # Storage node 2
  node3          # Storage node 3
  rebalancer     # Background repair service
```

### 14.2 Build Commands

```bash
# Build all services
docker compose build

# Build specific service
docker compose build gateway

# Build without cache (force rebuild)
docker compose build --no-cache gateway

# Clean Docker build cache
docker builder prune -f
```

### 14.3 Run Commands

```bash
# Start all services
docker compose up -d

# Start with logs
docker compose up

# Start specific services
docker compose up -d postgres redis gateway

# Force recreate containers
docker compose up -d --force-recreate gateway

# Stop all services
docker compose down

# Stop and remove volumes (CAUTION: deletes data!)
docker compose down -v
```

### 14.4 View Logs

```bash
# All services
docker compose logs -f

# Specific service
docker compose logs -f gateway

# Last 100 lines
docker compose logs --tail=100 gateway
```

### 14.5 Service Health Checks

```bash
# Check service status
docker compose ps

# Check gateway health
curl http://localhost:8080/health

# Check node health
curl http://localhost:9091/health

# Check PostgreSQL
docker compose exec postgres pg_isready -U cyxcloud

# Check Redis
docker compose exec redis redis-cli ping
```

### 14.6 Database Access

```bash
# Connect to PostgreSQL
docker compose exec postgres psql -U cyxcloud -d cyxcloud

# Useful queries
SELECT * FROM nodes;
SELECT * FROM files ORDER BY created_at DESC LIMIT 10;
SELECT * FROM chunk_replication_status WHERE health_status != 'healthy';
```

### 14.7 Volume Management

```bash
# List volumes
docker volume ls | grep cyx

# Inspect volume
docker volume inspect cyx_cloud_gateway-data

# Backup volume data
docker run --rm -v cyx_cloud_postgres-data:/data -v $(pwd):/backup \
    alpine tar cvf /backup/postgres-backup.tar /data
```

### 14.8 Dockerfile Reference

**Dockerfile.gateway.build:**
```dockerfile
FROM rust:1.75-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --package cyxcloud-gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 curl
COPY --from=builder /app/target/release/cyxcloud-gateway /usr/local/bin/
EXPOSE 8080 50052
CMD ["cyxcloud-gateway", "--http-addr", "0.0.0.0:8080", "--grpc-addr", "0.0.0.0:50052"]
```

**Dockerfile.node:**
```dockerfile
FROM rust:1.75-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --package cyxcloud-node

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 curl
COPY --from=builder /app/target/release/cyxcloud-node /usr/local/bin/
EXPOSE 50051 4001 9090
CMD ["cyxcloud-node"]
```

---

## 15. Security Considerations

### 15.1 Transport Layer Security

| Connection | Protocol | TLS Required |
|------------|----------|--------------|
| External HTTP | HTTPS | Production: Yes |
| External gRPC | gRPC over TLS | Production: Yes |
| Internal services | Plain TCP | Docker network only |
| WebSocket | WSS | Production: Yes |

### 15.2 Authentication Security

- **JWT Secret**: Shared between Gateway and CyxWiz API
  - Must be cryptographically random (32+ bytes)
  - Never commit to version control
  - Rotate periodically

- **Token Blacklist**: Logout invalidates tokens via Redis
- **Nonce Management**: Challenge nonces are single-use

### 15.3 Node Authentication

```
1. Node authenticates with CyxWiz API (email/password)
2. Receives JWT token
3. Uses JWT for Gateway registration
4. Gateway validates token signature
5. Node receives node-specific token (24h lifetime)
```

### 15.4 Data Security

| Feature | Status | Description |
|---------|--------|-------------|
| Encryption at rest | Optional | AES-256-GCM (client-side) |
| Encryption in transit | Production | TLS for all external connections |
| Content integrity | Always | Blake3 hash verification |
| Erasure coding | Always | Reed-Solomon 10+4 |

### 15.5 Rate Limiting

Recommended limits (to be implemented):

| Endpoint | Limit |
|----------|-------|
| `/auth/challenge` | 10/min per IP |
| `/auth/wallet` | 5/min per wallet |
| `/s3/*` (uploads) | Based on quota |
| `/ws` connections | 10 per user |

### 15.6 Input Validation

- **Wallet addresses**: Validate Base58, 32-44 characters
- **Bucket names**: Alphanumeric, 3-63 characters
- **Object keys**: Max 1024 characters, no path traversal
- **File sizes**: Enforce user quotas

### 15.7 Secrets Management

**Required secrets:**
```
JWT_SECRET              # JWT signing key
POSTGRES_PASSWORD       # Database password
CYXWIZ_PASSWORD         # Node auth password (per node)
GATEWAY_KEYPAIR_PATH    # Solana keypair (if blockchain enabled)
```

**Best practices:**
1. Use environment variables (not config files)
2. Use Docker secrets for production
3. Rotate secrets periodically
4. Monitor for leaked credentials

### 15.8 Network Security

**Docker network isolation:**
```yaml
networks:
  cyxcloud-net:
    driver: bridge
    ipam:
      config:
        - subnet: 172.28.0.0/16
```

**Firewall recommendations:**
- External: Only expose 8080 (HTTP) and 50052 (gRPC)
- Internal: All services can communicate within Docker network
- Production: Use reverse proxy (nginx/traefik) with TLS termination

---

## Appendix A: Quick Reference

### Common Commands

```bash
# Start the stack
docker compose up -d

# Check health
curl http://localhost:8080/health

# Upload a file
cyxcloud-cli upload --bucket test --key myfile.txt ./localfile.txt

# Download a file
cyxcloud-cli download --bucket test --key myfile.txt ./output.txt

# View nodes
docker compose exec postgres psql -U cyxcloud -c "SELECT peer_id, status, storage_used FROM nodes;"

# View file uploads
docker compose exec postgres psql -U cyxcloud -c "SELECT name, size_bytes, chunk_count FROM files ORDER BY created_at DESC LIMIT 5;"

# Rebuild gateway
docker compose build --no-cache gateway && docker compose up -d --force-recreate gateway
```

### Port Summary

```
8080  - Gateway HTTP (S3 API, Auth, WebSocket)
50052 - Gateway gRPC (NodeService, DataService)
50051 - Storage Node gRPC (ChunkService)
4001  - Storage Node libp2p
9090  - Storage Node metrics
5432  - PostgreSQL
6379  - Redis
3002  - CyxWiz API
```

### Environment Variables Summary

```bash
# Gateway
DATABASE_URL=postgres://user:pass@postgres:5432/cyxcloud
REDIS_URL=redis://redis:6379
JWT_SECRET=your-secret-here
SOLANA_RPC_URL=https://api.devnet.solana.com

# Storage Node
NODE_ID=node-1
CENTRAL_SERVER_ADDR=http://gateway:50052
STORAGE_CAPACITY_GB=100
CYXWIZ_API_URL=http://cyxwiz-api:3002
CYXWIZ_EMAIL=node@example.com
CYXWIZ_PASSWORD=password

# Rebalancer
SCAN_INTERVAL_SECS=300
REPAIR_PARALLELISM=4
MIN_REPLICAS=3
```

---

*Document generated: 2024-12-28*
*CyxCloud Version: 0.1.0*
