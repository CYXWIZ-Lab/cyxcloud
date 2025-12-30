# CyxCloud System Flow & Architecture

## Overview

CyxCloud uses a **hybrid architecture**:
- **Centralized coordination** for authentication, metadata, and node registry
- **Distributed data transfer** via libp2p for P2P chunk transfer between nodes

This balances the benefits of both approaches:
- Central server provides reliable user authentication, metadata consistency, and node discovery
- P2P transfer reduces bandwidth costs and enables direct node-to-node communication

---

## Component Roles

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CyxCloud Architecture                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐     ┌──────────────────┐     ┌──────────────────────────┐ │
│  │   Website    │     │   Rust REST API  │     │      PostgreSQL          │ │
│  │ (cyxwiz.com) │────▶│ (apps/api)       │────▶│   (User accounts,        │ │
│  │              │     │                  │     │    wallet addresses)     │ │
│  └──────────────┘     └──────────────────┘     └──────────────────────────┘ │
│         │                     │                            │                 │
│         │                     │ JWT                        │                 │
│         ▼                     ▼                            ▼                 │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         CyxCloud Gateway                              │   │
│  │                      (Central Coordination)                           │   │
│  │  • User authentication (validates JWT from REST API)                  │   │
│  │  • Metadata service (file index, chunk locations)                     │   │
│  │  • S3-compatible REST API                                             │   │
│  │  • WebSocket for real-time updates                                    │   │
│  │  • Node registry (tracks all storage nodes)                           │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│         │                                                                    │
│         │ gRPC + JWT                                                        │
│         ▼                                                                    │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                         Storage Nodes (cyxcloud-node)                 │   │
│  │                                                                       │   │
│  │  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐           │   │
│  │  │ Node A  │◀──▶│ Node B  │◀──▶│ Node C  │◀──▶│ Node D  │           │   │
│  │  └─────────┘    └─────────┘    └─────────┘    └─────────┘           │   │
│  │       ▲              ▲              ▲              ▲                 │   │
│  │       │              │              │              │                 │   │
│  │       └──────────────┴──────────────┴──────────────┘                 │   │
│  │                    libp2p P2P Network                                 │   │
│  │              (Kademlia DHT + Direct Transfer)                         │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                           CLI Tool                                    │   │
│  │  • User login (via REST API → JWT)                                    │   │
│  │  • Upload/Download files (via Gateway)                                │   │
│  │  • Local caching                                                      │   │
│  └──────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Authentication Flow

### 1. User Registration (Website)

```
User ──▶ Website ──▶ REST API ──▶ PostgreSQL
                         │
                         ├─ Create user account
                         ├─ Generate wallet address (Solana)
                         └─ Store in users table
```

### 2. User Login (CLI or Node)

```
CLI/Node ──▶ REST API (apps/api)
                  │
                  ├─ Validate credentials
                  ├─ Generate JWT token
                  │   {
                  │     "sub": "user_id",
                  │     "wallet": "GwLqe8...",
                  │     "exp": 1700000000,
                  │     "permissions": ["storage:read", "storage:write"]
                  │   }
                  └─ Return JWT to client

CLI/Node stores JWT locally for subsequent requests
```

### 3. JWT Usage

All subsequent requests include JWT in Authorization header:
```
Authorization: Bearer <jwt_token>
```

The Gateway validates JWT and extracts user context for operations.

---

## Storage Node Flow

### 1. Node Installation & Login

```
Operator downloads cyxcloud-node binary
    │
    ▼
cyxcloud-node --login
    │
    ├─ Opens browser or prompts for credentials
    ├─ Authenticates via REST API
    ├─ Receives JWT token
    └─ Stores JWT in config file (~/.cyxcloud/credentials)
```

### 2. Storage Allocation

```
cyxcloud-node allocate --space 100GB
    │
    ▼
Node ──gRPC + JWT──▶ Gateway
                         │
                         ├─ Validate JWT
                         ├─ Check node doesn't exist
                         ├─ Register node in PostgreSQL:
                         │   - node_id (UUID)
                         │   - peer_id (libp2p)
                         │   - grpc_address
                         │   - storage_total: 100GB
                         │   - storage_used: 0
                         │   - operator_wallet
                         │   - region, datacenter
                         └─ Return registration confirmation

Node starts:
  • gRPC server (for chunk operations)
  • libp2p swarm (for P2P discovery)
  • Heartbeat service (to Gateway)
  • Metrics endpoint
```

### 3. Node Heartbeat

```
Every 30 seconds:

Node ──gRPC──▶ Gateway
                  │
                  ├─ Update last_heartbeat
                  ├─ Report storage_used
                  ├─ Report active connections
                  └─ Check for pending jobs
```

---

## User Data Flow (CLI)

### 1. Upload File

```
cyxcloud upload myfile.dat mybucket/myfile.dat
    │
    ▼
CLI ──────────────────────────────────────▶ Gateway (S3 API)
                                                │
Step 1: Initiate Upload                         │
    ├─ Validate JWT                             │
    ├─ Check bucket exists                      │
    ├─ Create file record in metadata DB        │
    │   - file_id, path, size                   │
    │   - owner_id, bucket                      │
    │   - status: 'uploading'                   │
    └─ Return upload plan:                      │
        {                                       │
          "file_id": "uuid",                    │
          "chunks": [                           │
            {"index": 0, "size": 4MB, "nodes": ["nodeA", "nodeB", "nodeC"]},
            {"index": 1, "size": 4MB, "nodes": ["nodeB", "nodeC", "nodeD"]},
            ...
          ]
        }

Step 2: Upload Chunks (P2P or via Gateway)

CLI ───────▶ Erasure encode file (10 data + 4 parity)
    │
    ▼
For each shard:
    CLI ──gRPC──▶ Primary Node
                     │
                     ├─ Store chunk locally (RocksDB)
                     ├─ Report success to Gateway
                     └─ Replicate to secondary nodes (P2P)

Step 3: Complete Upload

CLI ──▶ Gateway: POST /complete-upload
            │
            ├─ Verify all chunks stored
            ├─ Update file status: 'complete'
            └─ Update chunk locations in DB
```

### 2. Download File

```
cyxcloud download mybucket/myfile.dat localfile.dat
    │
    ▼
CLI ──▶ Gateway: GET file metadata
            │
            ├─ Validate JWT
            ├─ Find file in metadata DB
            └─ Return chunk locations:
                {
                  "file_id": "uuid",
                  "size": 100MB,
                  "chunks": [
                    {"chunk_id": "abc...", "nodes": ["nodeA:50051", "nodeB:50051"]},
                    {"chunk_id": "def...", "nodes": ["nodeB:50051", "nodeC:50051"]},
                    ...
                  ]
                }
    │
    ▼
For each chunk:
    CLI ──gRPC──▶ Available Node
                     │
                     └─ Return chunk data
    │
    ▼
CLI: Erasure decode (need any 10 of 14 shards)
    │
    ▼
Write to local file
```

---

## libp2p Role (P2P Layer)

libp2p provides the **data plane** while the Gateway provides the **control plane**.

### What libp2p Handles:
1. **Peer Discovery** (Kademlia DHT)
   - Nodes find each other without central server
   - Useful for: direct transfers, rebalancing, health checks

2. **Direct Data Transfer**
   - Nodes transfer chunks directly (not through Gateway)
   - Reduces bandwidth on central infrastructure

3. **Replication**
   - When node receives chunk, it replicates to peers via P2P
   - Gateway only tracks locations, doesn't relay data

### What Gateway Handles:
1. **Authentication** - JWT validation
2. **Metadata** - File/chunk index in PostgreSQL
3. **Node Registry** - Which nodes are online and available
4. **Placement Decisions** - Which nodes should store new chunks
5. **Consistency** - Ensuring correct replica count

### Hybrid Lookup Flow

```
User wants chunk "abc123"
    │
    ▼
Option A: Ask Gateway (preferred for reliability)
    CLI ──▶ Gateway: "Where is chunk abc123?"
                ├─ Query PostgreSQL chunk_locations
                └─ Return: ["nodeA:50051", "nodeB:50051"]
    CLI ──gRPC──▶ nodeA: "Give me chunk abc123"

Option B: Direct P2P lookup (for resilience)
    CLI ──libp2p DHT──▶ "Who has chunk abc123?"
                            │
                            └─ DHT returns peer addresses
    CLI ──libp2p──▶ peer: request chunk
```

---

## Payment Flow

### Storage Miner Earnings

```
Every billing period (e.g., hourly):

Gateway calculates:
    ├─ For each node:
    │   - storage_provided (GB-hours)
    │   - bandwidth_served (GB)
    │   - uptime_percentage
    │
    ├─ Calculate CYXWIZ earnings:
    │   earnings = base_rate * storage + bandwidth_rate * bandwidth
    │
    └─ Queue payment on Solana:
        transfer(platform_wallet → node_operator_wallet, earnings)
```

### User Storage Costs

```
User uploads 100GB, selects Pro plan (40 CYXWIZ/month)
    │
    ▼
Website ──▶ Create subscription in DB
    │
    ▼
Monthly:
    Charge wallet: 40 CYXWIZ
    │
    ├─ 90% distributed to storage nodes
    └─ 10% platform fee
```

---

## Security Considerations

### JWT Token Structure

```json
{
  "sub": "user-uuid",
  "wallet": "GwLqe8XZ8R4kpXvGJJ9kVpWfVb8KiL4RMxKqKn3D6W3j",
  "exp": 1700000000,
  "iat": 1699900000,
  "permissions": [
    "storage:read",
    "storage:write",
    "node:register"  // Only for node operators
  ]
}
```

### Token Validation

1. **REST API** (apps/api) issues tokens after password/wallet auth
2. **Gateway** validates tokens for all operations
3. **Nodes** validate tokens for P2P requests (optional, for premium features)

### Node Authentication

Nodes authenticate to Gateway with:
1. JWT from operator's account
2. Node-specific keypair (generated on first run)
3. Signed heartbeats prevent impersonation

---

## Summary: Central vs P2P

| Aspect | Central (Gateway) | P2P (libp2p) |
|--------|-------------------|--------------|
| Authentication | ✅ JWT validation | ❌ |
| User accounts | ✅ PostgreSQL | ❌ |
| File metadata | ✅ PostgreSQL | ❌ |
| Chunk locations | ✅ PostgreSQL (source of truth) | DHT (cache) |
| Node registry | ✅ PostgreSQL | DHT (discovery) |
| Upload coordination | ✅ Placement decisions | ❌ |
| Chunk transfer | ⚡ Optional relay | ✅ Primary |
| Replication | ⚡ Triggers | ✅ Executes |
| Billing | ✅ | ❌ |

**Key insight**: Gateway is the **control plane**, libp2p is the **data plane**.

---

## Implementation Status

| Component | Status | Notes |
|-----------|--------|-------|
| REST API (apps/api) | ✅ Exists | User auth, JWT |
| Gateway | 🔶 80% | Metadata integrated, chunk relay TODO |
| Node (cyxcloud-node) | ✅ Complete | gRPC server, health checks |
| Metadata Service | ✅ Complete | PostgreSQL + Redis |
| CLI | 🔶 70% | Upload/download TODO |
| libp2p Integration | 🔶 60% | Discovery works, transfer TODO |
| Payment System | ❌ TODO | Solana integration |
