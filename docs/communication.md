# CyxCloud Communication Architecture

This document describes the complete communication flow between all CyxCloud components:
- Storage Nodes (miners)
- Gateway (central coordinator)
- CLI (command-line interface)
- Website (web frontend)
- CyxWiz API (authentication/user management)

---

## Communication Ports & Services

| Service | Port | Protocol | Purpose |
|---------|------|----------|---------|
| **CyxWiz API** | 3002 | HTTP REST | User auth, machine registration, wallets |
| **Gateway (HTTP)** | 8080 | HTTP/S3/WS | S3-compatible API, health, auth, WebSocket |
| **Gateway (gRPC)** | 50052 | gRPC | Node registration, data streaming |
| **Storage Node gRPC** | 50051 | gRPC | Chunk storage/retrieval |
| **Storage Node P2P** | 4001 | libp2p | Node-to-node communication |
| **Node Metrics** | 9090 | HTTP | Prometheus metrics, health checks |
| **PostgreSQL** | 5432 | TCP | Metadata database |
| **Redis** | 6379 | TCP | Caching layer |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          CyxCloud Architecture                               │
└─────────────────────────────────────────────────────────────────────────────┘

                           ┌─────────────────┐
                           │   CyxWiz API    │
                           │   (Port 3002)   │
                           │  REST + Auth    │
                           └────────┬────────┘
                                    │
            ┌───────────────────────┼───────────────────────┐
            │                       │                       │
            ▼                       ▼                       ▼
    ┌───────────────┐       ┌───────────────┐       ┌───────────────┐
    │ Storage Node  │       │    Gateway    │       │   Website /   │
    │   (Miner)     │       │ (8080/50052)  │       │     CLI       │
    └───────┬───────┘       └───────┬───────┘       └───────┬───────┘
            │                       │                       │
            │      ┌────────────────┼────────────────┐      │
            │      │                │                │      │
            ▼      ▼                ▼                ▼      ▼
    ┌──────────────────────────────────────────────────────────────┐
    │                     Storage Network                           │
    │   Node1 ◄──── libp2p ────► Node2 ◄──── libp2p ────► Node3    │
    │  (50061)                   (50062)                  (50063)   │
    └──────────────────────────────────────────────────────────────┘
```

---

## Authentication Flow

### Two-Stage Authentication

CyxCloud uses a two-stage authentication model:

1. **Stage 1: CyxWiz API Login (Port 3002)**
   - User/Node logs in with email/password
   - Receives JWT token + user info (wallet address, etc.)

2. **Stage 2: Gateway Authentication (Port 50052)**
   - Uses JWT token from Stage 1
   - Gateway validates token (shared JWT_SECRET with CyxWiz API)
   - Node can register and send heartbeats

### Authentication Flow Diagram

```
  STORAGE MINERS (Node)                           USERS (CLI/Website)
  ──────────────────────                          ────────────────────

  ┌─────────────┐                                 ┌─────────────┐
  │ Download &  │                                 │ Website or  │
  │ Run Node    │                                 │ CLI Login   │
  └─────┬───────┘                                 └──────┬──────┘
        │                                                │
        ▼                                                ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │                    CyxWiz API (Port 3002)                        │
  │  POST /api/auth/login  →  Returns JWT token + User info          │
  │  - user_id, email, username                                      │
  │  - cyx_wallet (public_key), external_wallet                      │
  └─────────────────────────────────────────────────────────────────┘
        │                                                │
        │ JWT Token                                      │ JWT Token
        ▼                                                ▼
  ┌─────────────┐                                 ┌─────────────┐
  │ Machine     │                                 │ Access      │
  │ Registration│                                 │ User Data   │
  │ POST /api/  │                                 │ Download    │
  │ machines/   │                                 │ Files       │
  │ register    │                                 └──────┬──────┘
  └─────┬───────┘                                        │
        │ api_key + machine_id                           │
        ▼                                                ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │                    Gateway (Port 8080/50052)                     │
  │                                                                  │
  │  HTTP (8080):                     gRPC (50052):                  │
  │  - S3 API (/s3/*)                 - NodeService.Register         │
  │  - Auth API (/api/v1/auth)        - NodeService.Heartbeat        │
  │  - WebSocket (/ws)                - DataService.StreamData       │
  │  - Health (/health)                                              │
  │                                                                  │
  │  JWT Token validated from CyxWiz API (shared secret)             │
  └─────────────────────────────────────────────────────────────────┘
        │                                                │
        │ Node registers with                            │
        │ JWT token                                      │
        ▼                                                ▼
  ┌──────────────┐                               ┌──────────────┐
  │Storage Node 1│◄──────────libp2p─────────────►│Storage Node 2│
  │  Port 50051  │           (4001)              │  Port 50051  │
  │  Port 4001   │                               │  Port 4001   │
  │  Port 9090   │                               │  Port 9090   │
  └──────────────┘                               └──────────────┘
```

---

## Storage Miner (Node) Flow

### Complete Node Startup Sequence

```
Node Startup:
┌──────────────────────────────────────────────────────────────────┐
│ 1. CONFIGURATION                                                  │
│    Load config (node.toml) or use defaults                       │
│    Check for saved credentials (.cyxwiz_credentials.json)        │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│ 2. AUTHENTICATION (CyxWiz API - Port 3002)                       │
│    If no credentials:                                            │
│    - Interactive: Prompt email/password (if TTY)                 │
│    - Non-interactive: Use CYXWIZ_EMAIL/CYXWIZ_PASSWORD env vars  │
│                                                                  │
│    POST /api/auth/login                                          │
│    Response: { token, user: { id, wallet, ... } }                │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│ 3. MACHINE REGISTRATION (CyxWiz API - Port 3002)                 │
│    POST /api/machines/register                                   │
│    Payload: { owner_id, hostname, os, hardware }                 │
│    Response: { machine_id, api_key, compute_units }              │
│                                                                  │
│    Detected hardware includes:                                   │
│    - CPUs (cores, threads, frequency)                            │
│    - GPUs (name, VRAM, CUDA version)                             │
│    - RAM (total MB)                                              │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│ 4. GATEWAY REGISTRATION (Gateway gRPC - Port 50052)              │
│    Use JWT token from Step 2                                     │
│    NodeService.RegisterNode()                                    │
│    - Node ID, capacity, region, wallet address                   │
│                                                                  │
│    >>> GATEWAY RESERVES 2GB FROM NODE STORAGE <<<                │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│ 5. HEARTBEAT LOOPS                                               │
│                                                                  │
│    CyxWiz API Heartbeat (every 30s):                             │
│    POST /api/machines/{machine_id}/heartbeat                     │
│    - ip_address, version, is_online, current_load                │
│                                                                  │
│    Gateway Heartbeat (every 30s):                                │
│    NodeService.Heartbeat()                                       │
│    - node_id, status, metrics                                    │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│ 6. READY FOR CHUNK STORAGE                                       │
│    gRPC Server listening on Port 50051                           │
│    - StoreChunk, GetChunk, DeleteChunk                           │
│    - VerifyChunk (for integrity checks)                          │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2GB Gateway Reservation

### Overview

When a Storage Node registers with the Gateway, the Gateway **reserves 2GB** from the node's allocated storage capacity. This reserved space is used for:

1. **System Metadata**: Chunk location indexes, file metadata cache
2. **Erasure Coding Overhead**: Parity shards for data recovery
3. **Rebalancing Buffer**: Temporary storage during chunk migration
4. **Health Check Data**: Verification chunks for node health

### Reservation Flow

```
┌──────────────────────────────────────────────────────────────────┐
│                   NODE REGISTRATION FLOW                          │
└──────────────────────────────────────────────────────────────────┘

  Storage Node                      Gateway                    Database
      │                                │                           │
      │  RegisterNode(capacity=100GB)  │                           │
      ├───────────────────────────────►│                           │
      │                                │                           │
      │                                │  Reserve 2GB (system)     │
      │                                ├──────────────────────────►│
      │                                │                           │
      │                                │  Record available: 98GB   │
      │                                ├──────────────────────────►│
      │                                │                           │
      │  RegisterNodeResponse          │                           │
      │  { available_capacity: 98GB }  │                           │
      │◄───────────────────────────────┤                           │
      │                                │                           │


┌──────────────────────────────────────────────────────────────────┐
│                   STORAGE ALLOCATION BREAKDOWN                    │
└──────────────────────────────────────────────────────────────────┘

    Node Allocated: 100 GB
    ┌──────────────────────────────────────────────────────────────┐
    │                                                              │
    │  ┌────────────┐  ┌─────────────────────────────────────────┐ │
    │  │ Reserved   │  │         User Data Storage               │ │
    │  │   2 GB     │  │              98 GB                      │ │
    │  │            │  │                                         │ │
    │  │ - Metadata │  │  - Chunks (after erasure coding)        │ │
    │  │ - Parity   │  │  - User files                           │ │
    │  │ - Buffer   │  │  - Replicated data                      │ │
    │  └────────────┘  └─────────────────────────────────────────┘ │
    │                                                              │
    └──────────────────────────────────────────────────────────────┘
```

### Reserved Space Usage

| Category | Size | Purpose |
|----------|------|---------|
| **Metadata Index** | 500 MB | Local chunk location cache |
| **Parity Buffer** | 500 MB | Temporary erasure coding parity |
| **Rebalance Buffer** | 500 MB | Chunk migration staging |
| **Health Data** | 256 MB | Verification chunks |
| **Logs & Metrics** | 256 MB | Performance data, error logs |

### Implementation Details

The reservation is enforced at two levels:

1. **Gateway Level**: Gateway sets `storage_reserved = 2GB` when node registers
2. **Database Level**: PostgreSQL stores and tracks reserved vs available storage

**Key Files:**
- `cyxcloud-gateway/src/grpc_api.rs` - Constants and registration logic
- `cyxcloud-metadata/src/models.rs` - Node model with `storage_reserved` field
- `cyxcloud-metadata/migrations/003_storage_reservation.sql` - Database schema

```rust
// cyxcloud-gateway/src/grpc_api.rs

/// Gateway-reserved storage per node (2 GB)
pub const GATEWAY_RESERVED_BYTES: i64 = 2 * 1024 * 1024 * 1024; // 2 GB

/// Minimum storage a node must provide (after reservation)
pub const MIN_ALLOCATABLE_BYTES: i64 = 1 * 1024 * 1024 * 1024; // 1 GB

// During node registration:
let create_node = CreateNode {
    peer_id: node_id.clone(),
    storage_total: capacity.storage_total as i64,
    storage_reserved: GATEWAY_RESERVED_BYTES,  // Always 2GB
    // ...
};
```

**Validation**: Nodes must provide at least 3GB total storage (2GB reserved + 1GB allocatable):

```rust
let allocatable = capacity.storage_total - GATEWAY_RESERVED_BYTES;
if allocatable < MIN_ALLOCATABLE_BYTES {
    return Err("Insufficient storage");
}
```

**Database View** (node_storage_summary):
```sql
storage_available = storage_total - storage_reserved - storage_used
storage_allocatable = storage_total - storage_reserved
utilization_percent = storage_used / storage_allocatable * 100
```

### Payment Implications

- Miners are **paid for reserved storage** even though it's not user data
- This incentivizes nodes to maintain system health
- Payment formula: `payment = (user_storage + reserved_storage) * rate`

---

## CLI User Flow

### Available Commands

```bash
# Basic operations
cyxcloud --gateway http://localhost:8080 upload <path> --bucket <name>
cyxcloud --gateway http://localhost:8080 download <bucket> --key <key>
cyxcloud --gateway http://localhost:8080 list <bucket> --prefix <prefix>
cyxcloud --gateway http://localhost:8080 delete <bucket> <key>
cyxcloud --gateway http://localhost:8080 status

# Node management (TODO)
cyxcloud node status
cyxcloud node start --storage-gb 100
cyxcloud node stop
```

### CLI → Gateway Communication

```
┌─────────────────────────────────────────────────────────────────┐
│                     CLI UPLOAD FLOW                              │
└─────────────────────────────────────────────────────────────────┘

  CLI                              Gateway                  Storage Nodes
   │                                  │                           │
   │  PUT /s3/{bucket}/{key}          │                           │
   │  Content-Type: application/...   │                           │
   │  [file data]                     │                           │
   ├─────────────────────────────────►│                           │
   │                                  │                           │
   │                                  │  1. Split into chunks     │
   │                                  │  2. Erasure encode        │
   │                                  │     (10 data + 4 parity)  │
   │                                  │                           │
   │                                  │  StoreChunk() x N         │
   │                                  ├──────────────────────────►│
   │                                  │                           │
   │                                  │  3. Record locations      │
   │                                  │     in PostgreSQL         │
   │                                  │                           │
   │  200 OK                          │                           │
   │  ETag: "abc123..."               │                           │
   │◄─────────────────────────────────┤                           │
   │                                  │                           │
```

### CLI Authentication (Implemented)

```bash
# Login with email/password
cyxcloud login
> Email: user@example.com
> Password: ********
> ✓ Logged in successfully!
> Welcome, user@example.com

# Check current user
cyxcloud whoami

# Logout
cyxcloud logout

# Upload (uses saved token automatically)
cyxcloud upload myfile.dat --bucket mybucket
```

**Credential Storage:**
- Config: `~/.cyxcloud/config.toml`
- CLI credentials: `~/.cyxcloud/credentials.json`
- Node credentials: `~/.cyxcloud/node_credentials.json`

---

## Website Communication

### Endpoints Used

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/v1/auth/challenge` | POST | Get wallet sign challenge |
| `/api/v1/auth/wallet` | POST | Verify wallet signature |
| `/s3/{bucket}` | GET | List objects |
| `/s3/{bucket}/{key}` | GET/PUT/DELETE | Object operations |
| `/ws` | WebSocket | Real-time updates |
| `/health` | GET | Health check |
| `/version` | GET | Gateway version |

### WebSocket Events

The Gateway provides real-time updates via WebSocket:

```
┌─────────────────────────────────────────────────────────────────┐
│                    WEBSOCKET CONNECTION                          │
└─────────────────────────────────────────────────────────────────┘

  Website                           Gateway
     │                                 │
     │  GET /ws?topics=file,cluster    │
     │  Upgrade: websocket             │
     ├────────────────────────────────►│
     │                                 │
     │  101 Switching Protocols        │
     │◄────────────────────────────────┤
     │                                 │
     │  ══════ WebSocket Open ══════   │
     │                                 │
     │  {"type":"Heartbeat",           │
     │   "data":{"timestamp":1234}}    │
     │◄────────────────────────────────┤  (every 30s)
     │                                 │
     │  {"type":"FileCreated",         │
     │   "data":{"bucket":"x",         │
     │           "key":"y",            │
     │           "size":1024}}         │
     │◄────────────────────────────────┤  (on upload)
     │                                 │
     │  {"type":"NodeHealthChanged",   │
     │   "data":{"node_id":"n1",       │
     │           "status":"online"}}   │
     │◄────────────────────────────────┤  (on node change)
     │                                 │
```

### WebSocket Event Types

```rust
pub enum Event {
    // File events
    FileCreated { bucket, key, size },
    FileDeleted { bucket, key },
    UploadProgress { upload_id, bytes_uploaded, total_bytes, percent },
    UploadComplete { upload_id, bucket, key, etag },
    DownloadProgress { download_id, bytes_downloaded, total_bytes, percent },

    // Cluster events
    NodeJoined { node_id, address },
    NodeLeft { node_id, reason },
    NodeHealthChanged { node_id, status, details },

    // Replication events
    ReplicationStarted { chunk_id, source_node, target_node },
    ReplicationComplete { chunk_id, target_node },
    ReplicationFailed { chunk_id, error },

    // Job events (for CyxWiz ML integration)
    JobStatusChanged { job_id, status, progress },

    // System events
    Heartbeat { timestamp },
    Error { code, message },
}
```

### Topic Filtering

Clients can subscribe to specific event categories:

```javascript
// Subscribe to all events
const ws = new WebSocket("ws://gateway:8080/ws");

// Subscribe to specific topics
const ws = new WebSocket("ws://gateway:8080/ws?topics=file,cluster");

// Available topics:
// - file: FileCreated, FileDeleted, UploadProgress, etc.
// - cluster: NodeJoined, NodeLeft, NodeHealthChanged
// - replication: ReplicationStarted, ReplicationComplete, ReplicationFailed
// - job: JobStatusChanged
// - system: Heartbeat, Error
```

---

## Environment Variables

### Storage Node

| Variable | Default | Description |
|----------|---------|-------------|
| `CYXWIZ_API_URL` | `http://localhost:3002` | CyxWiz API URL |
| `CYXWIZ_EMAIL` | - | Email for non-interactive login |
| `CYXWIZ_PASSWORD` | - | Password for non-interactive login |
| `RUST_LOG` | `info` | Log level |
| `NODE_ID` | auto-generated | Unique node identifier |
| `GRPC_HOST` | `0.0.0.0` | gRPC bind address |
| `GRPC_PORT` | `50051` | gRPC port |
| `LIBP2P_PORT` | `4001` | P2P port |
| `METRICS_PORT` | `9090` | Metrics/health port |
| `STORAGE_PATH` | `./data` | Chunk storage directory |
| `STORAGE_CAPACITY_GB` | `0` (unlimited) | Max storage allocation |
| `CENTRAL_SERVER_ADDR` | `http://localhost:50052` | Gateway gRPC address |

### Gateway

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | - | PostgreSQL connection string |
| `REDIS_URL` | - | Redis connection string |
| `JWT_SECRET` | random | Shared with CyxWiz API |
| `JWT_SKIP_ISSUER_VALIDATION` | `true` | Accept tokens from CyxWiz API |
| `JWT_ACCEPTED_ISSUERS` | `cyxwiz-api` | Comma-separated list |
| `SOLANA_RPC_URL` | `https://api.devnet.solana.com` | Blockchain RPC |
| `ENABLE_BLOCKCHAIN` | `false` | Enable Solana integration |
| `GATEWAY_KEYPAIR_PATH` | - | Solana authority keypair |

### CLI

| Variable | Default | Description |
|----------|---------|-------------|
| `CYXCLOUD_GATEWAY` | `http://localhost:8080` | Default gateway URL |
| `CYXCLOUD_TOKEN` | - | Saved authentication token |

---

## JWT Token Flow

### Token Structure

```json
{
  "sub": "user-id-uuid",
  "exp": 1700000000,
  "iat": 1699900000,
  "nbf": 1699900000,
  "jti": "unique-token-id",
  "user_type": "user|node|api_key",
  "wallet": "optional-solana-pubkey",
  "permissions": ["storage:read", "storage:write"]
}
```

### Cross-Service Token Sharing

```
┌──────────────────────────────────────────────────────────────────┐
│                    TOKEN SHARING ARCHITECTURE                     │
└──────────────────────────────────────────────────────────────────┘

  CyxWiz API (3002)                    Gateway (8080/50052)
  ─────────────────                    ────────────────────

  JWT_SECRET=shared_secret ───────────► JWT_SECRET=shared_secret
  JWT_ISSUER=cyxwiz-api                 JWT_ACCEPTED_ISSUERS=cyxwiz-api
                                        JWT_SKIP_ISSUER_VALIDATION=true

  Generates tokens ◄─────────────────── Validates tokens
  (on login)                            (skip issuer check)
```

### Token Lifetime

| Token Type | Lifetime | Use Case |
|------------|----------|----------|
| Access | 1 hour | Short-lived user sessions |
| Refresh | 7 days | Token renewal |
| Node | 24 hours | Node authentication |
| API Key | 1 year | Long-lived programmatic access |

---

## Payment Flow

### Storage Payment Model

```
┌──────────────────────────────────────────────────────────────────┐
│                     PAYMENT ARCHITECTURE                          │
└──────────────────────────────────────────────────────────────────┘

  User uploads data
        │
        ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  1. Check quota (blockchain subscription)                       │
  │     - storage_quota_bytes vs storage_used_bytes                 │
  │     - Reject if quota exceeded                                  │
  └─────────────────────────────────────────────────────────────────┘
        │
        ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  2. Store data (distributed across nodes)                       │
  │     - Erasure coding (10 data + 4 parity shards)                │
  │     - Record locations in metadata DB                           │
  └─────────────────────────────────────────────────────────────────┘
        │
        ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  3. Update blockchain usage                                     │
  │     - Increment storage_used_bytes                              │
  │     - Add bandwidth_used_bytes                                  │
  └─────────────────────────────────────────────────────────────────┘
        │
        ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │  4. Epoch reward calculation (Gateway → Blockchain)             │
  │     - Calculate node contribution                               │
  │     - Allocate rewards: 90% node, 10% platform                  │
  └─────────────────────────────────────────────────────────────────┘
```

### Subscription Plans

| Plan | Storage | Bandwidth | Price/Month |
|------|---------|-----------|-------------|
| Free | 5 GB | 10 GB | $0 |
| Starter | 100 GB | 500 GB | $9.99 |
| Pro | 1 TB | 5 TB | $49.99 |
| Enterprise | Custom | Custom | Contact |

---

## Erasure Coding

CyxCloud uses Reed-Solomon erasure coding for data durability:

```
┌──────────────────────────────────────────────────────────────────┐
│                    ERASURE CODING (10+4)                          │
└──────────────────────────────────────────────────────────────────┘

  Original File (1 MB)
  ┌────────────────────────────────────────────────────────────────┐
  │                          DATA                                  │
  └────────────────────────────────────────────────────────────────┘
                              │
                              ▼ Split into 10 data shards
  ┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐
  │ D1 ││ D2 ││ D3 ││ D4 ││ D5 ││ D6 ││ D7 ││ D8 ││ D9 ││D10 │
  └────┘└────┘└────┘└────┘└────┘└────┘└────┘└────┘└────┘└────┘
                              │
                              ▼ Generate 4 parity shards
  ┌────┐┌────┐┌────┐┌────┐
  │ P1 ││ P2 ││ P3 ││ P4 │
  └────┘└────┘└────┘└────┘
                              │
                              ▼ Distribute to 14 nodes
  ┌──────────────────────────────────────────────────────────────┐
  │  Node1: D1    Node5: D5     Node9: D9     Node13: P3         │
  │  Node2: D2    Node6: D6     Node10: D10   Node14: P4         │
  │  Node3: D3    Node7: D7     Node11: P1                       │
  │  Node4: D4    Node8: D8     Node12: P2                       │
  └──────────────────────────────────────────────────────────────┘

  Recovery: Can recover from ANY 4 node failures
  (Need only 10 of 14 shards to reconstruct data)
```

---

## Docker Compose Configuration

### Service Dependencies

```yaml
services:
  postgres:     # Standalone, starts first
  redis:        # Standalone, starts first
  gateway:      # Depends on: postgres, redis
  node1:        # Depends on: gateway
  node2:        # Depends on: gateway, node1
  node3:        # Depends on: gateway, node1, node2
  rebalancer:   # Depends on: postgres, node1, node2, node3
```

### Quick Start

```bash
# Start all services
cd cyx_cloud
docker compose up -d --build

# Check health
curl http://localhost:8080/health

# View logs
docker compose logs -f gateway

# Stop all services
docker compose down
```

---

## Troubleshooting

### Node Registration Fails

```bash
# Check if CyxWiz API is reachable
curl http://localhost:3002/health

# Check node logs
docker logs cyxcloud-node1

# Verify environment variables
docker exec cyxcloud-node1 env | grep CYXWIZ
```

### Gateway Authentication Issues

```bash
# Verify JWT_SECRET matches across services
echo $JWT_SECRET

# Check gateway logs for token validation errors
docker logs cyxcloud-gateway 2>&1 | grep -i "token\|auth"
```

### WebSocket Connection Issues

```bash
# Test WebSocket with wscat
wscat -c ws://localhost:8080/ws

# With topics
wscat -c "ws://localhost:8080/ws?topics=file,cluster"
```

---

## Future Enhancements

1. ~~**CLI Authentication**: Add `cyxcloud login` command with token persistence~~ ✅ DONE
2. ~~**Wallet Login**: Solana wallet signature authentication~~ ✅ NOT NEEDED (wallet included in user profile on login)
3. **Real-time Sync**: Bi-directional file sync via WebSocket
4. **Mobile SDK**: iOS/Android libraries for storage access
5. **Multi-region**: Cross-datacenter replication
6. **Encryption**: Client-side encryption before upload
