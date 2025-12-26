# CyxCloud - Decentralized Storage Platform

CyxCloud is a decentralized storage network built on Solana blockchain, providing secure, redundant, and cost-effective cloud storage through distributed nodes.

## Overview

CyxCloud distributes data across multiple storage nodes using Reed-Solomon erasure coding, ensuring data availability even if some nodes go offline. Node operators stake CYXWIZ tokens and earn rewards for providing reliable storage.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         CyxCloud Architecture                            │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│   Users                    Gateway                     Storage Nodes     │
│  ┌─────┐                 ┌─────────┐                 ┌─────────────┐    │
│  │ CLI │───S3 API───────▶│ Gateway │────gRPC────────▶│   Node 1    │    │
│  │ SDK │                 │  (Rust) │                 │   Node 2    │    │
│  │ Web │                 └────┬────┘                 │   Node 3    │    │
│  └─────┘                      │                      │    ...      │    │
│                               │                      └──────┬──────┘    │
│                               ▼                             │           │
│                    ┌──────────────────┐              libp2p DHT         │
│                    │   PostgreSQL     │                     │           │
│                    │   + Redis        │◀────────────────────┘           │
│                    └──────────────────┘                                 │
│                               │                                         │
│                               ▼                                         │
│                    ┌──────────────────┐                                 │
│                    │ Solana Blockchain │                                │
│                    │  (CYXWIZ Token)   │                                │
│                    └──────────────────┘                                 │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

## Features

- **S3-Compatible API** - Drop-in replacement for Amazon S3
- **Erasure Coding** - Reed-Solomon 10+4 redundancy (survives 4 node failures)
- **Decentralized** - No single point of failure
- **Blockchain Payments** - CYXWIZ token for subscriptions and rewards (fully implemented)
- **Proof-of-Storage** - Cryptographic verification of stored data
- **Automatic Rebalancing** - Self-healing network

## Components

| Component | Description | Port |
|-----------|-------------|------|
| **cyxcloud-gateway** | S3 API, request routing, blockchain integration | 8080 (HTTP), 50051 (gRPC) |
| **cyxcloud-node** | Storage node, chunk storage, P2P networking | 50052 (gRPC), 4001 (libp2p) |
| **cyxcloud-cli** | Command-line client for file operations | - |

## Blockchain Integration (Fully Implemented)

All Solana programs are deployed and operational on Devnet (deployed 2025-12-22):

| Program | Address | Description |
|---------|---------|-------------|
| **Storage Subscription** | `HZhWDJVkkUuHrgqkNb9bYxiCfqtnv8cnus9UQt843Fro` | User storage plans |
| **Storage Node Registry** | `AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z` | Node registration & staking |
| **Storage Payment Pool** | `4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX` | Reward distribution |
| **Job Escrow** | `3sTCk7gVqj5RU8JECmY2zDjGpFKYZncsM9KqYFZBkkM9` | Compute job payments |
| **Node Registry** | `H15XzFvYpGqm9aH66n64B4Ld7CtZNSaFSftN8kGPQhCz` | Compute node registry |

**Token:** CYXWIZ - `Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi`

**Authority:** `4Y5HWB9W9SELq3Yoyf7mK7KF5kTbuaGxd2BMvn3AyAG8`

## Storage Plans

| Plan | Storage | Monthly Price | Yearly Price |
|------|---------|---------------|--------------|
| Free | 5 GB | 0 CYXWIZ | - |
| Starter | 100 GB | 10 CYXWIZ | 96 CYXWIZ (20% off) |
| Pro | 1 TB | 50 CYXWIZ | 480 CYXWIZ (20% off) |
| Enterprise | 10 TB | 200 CYXWIZ | 1,920 CYXWIZ (20% off) |

## Node Economics

- **Minimum Stake:** 500 CYXWIZ
- **Unstake Lockup:** 7 days
- **Epoch Duration:** 7 days (weekly rewards)

**Reward Distribution:**
- 85% to Storage Nodes (weighted by storage, uptime, speed)
- 10% to Platform Treasury
- 5% to Community Fund

**Slashing Penalties:**
| Violation | Penalty |
|-----------|---------|
| Failed Proofs (3x) | 15% of stake |
| Extended Downtime | 5% of stake |
| Data Loss | 10% of stake |
| Corrupted Data | 50% of stake |

## Quick Start

### Prerequisites

- Rust 1.75+
- PostgreSQL 14+
- Redis 7+
- Solana CLI (for blockchain operations)

### Build

```bash
cd cyx_cloud

# Build all components
cargo build --release

# Build with blockchain features
cargo build --release --features blockchain
```

### Run Gateway

```bash
# Set environment variables
export DATABASE_URL="postgres://user:pass@localhost/cyxcloud"
export REDIS_URL="redis://localhost:6379"
export SOLANA_RPC_URL="https://api.devnet.solana.com"

# Run gateway
cargo run --release -p cyxcloud-gateway
```

### Run Storage Node

```bash
# Configure node
export NODE_ID="my-node-001"
export STORAGE_PATH="/data/cyxcloud"
export GRPC_PORT=50052
export LIBP2P_PORT=4001

# Run node
cargo run --release -p cyxcloud-node
```

### CLI Usage

```bash
# Configure endpoint
export CYXCLOUD_ENDPOINT="http://localhost:8080"

# Upload file
cyxcloud put my-bucket/file.txt ./local-file.txt

# Download file
cyxcloud get my-bucket/file.txt ./downloaded.txt

# List files
cyxcloud ls my-bucket/

# Delete file
cyxcloud rm my-bucket/file.txt
```

## Documentation

- **[Usage Guide](USAGE.md)** - Detailed build, run, and API documentation
- **[Architecture](ARCHITECTURE.md)** - System design and data flow
- **[Vision](VISION.md)** - Project roadmap and goals
- **[Infrastructure](infrastructure.md)** - Deployment and scaling

## Blockchain Scripts

Scripts for interacting with deployed Solana programs:

```bash
cd ../cyxwiz-blockchain

# Read payment pool state
npx ts-node scripts/read-pool.ts

# Read storage node registry
npx ts-node scripts/read-registry.ts

# Read user subscription
npx ts-node scripts/read-subscription.ts

# Create subscription (Free tier)
npx ts-node scripts/create-subscription.ts

# Register storage node
npx ts-node scripts/register-node.ts
```

## Initialized On-Chain Accounts

| Account | Address | Type |
|---------|---------|------|
| Payment Pool | `55KoVNy9GT9JyqM8PQD3Rre5Tzq8oxTXqCxxwuxzEpYX` | Global pool state |
| Pool Token Account | `JE42Lhns8F7MSgrRsf5UsqRMYwHgMyNbTYZiHXd3uEdK` | Pool's CYXWIZ holder |
| Node Registry | `33Wc6jiXD8aZtn6zXo2RYYbDVdeFLaQPSCVihsQ2ct5P` | Global registry state |
| Test Subscription | `HDBQBMbHeMdE8ibLdXetFHQthgsvPmou6b5uT5hWz2cd` | Free tier subscription |
| Test Node | `4RpeQ5TPZoE5e5Pct591wQXKrxpZ98JVc8QFwmEFn3Y9` | cyxcloud-node-001 |

## API Endpoints

### S3-Compatible (Gateway)

```
PUT    /bucket/key          - Upload object
GET    /bucket/key          - Download object
DELETE /bucket/key          - Delete object
GET    /bucket?list-type=2  - List objects
HEAD   /bucket/key          - Get object metadata
```

### gRPC Services

- `StorageService` - Chunk upload/download
- `HealthService` - Node health checks
- `NodeService` - Node registration/heartbeat

### WebSocket

```
ws://gateway:8080/ws - Real-time events (upload progress, node status)
```

## Environment Variables

### Gateway

| Variable | Default | Description |
|----------|---------|-------------|
| `HTTP_PORT` | 8080 | S3 API port |
| `GRPC_PORT` | 50051 | gRPC port |
| `DATABASE_URL` | - | PostgreSQL connection |
| `REDIS_URL` | - | Redis connection |
| `SOLANA_RPC_URL` | devnet | Solana RPC endpoint |
| `GATEWAY_KEYPAIR_PATH` | ~/.config/solana/id.json | Solana keypair |

### Storage Node

| Variable | Default | Description |
|----------|---------|-------------|
| `NODE_ID` | - | Unique node identifier |
| `STORAGE_PATH` | ./data | Local storage directory |
| `GRPC_PORT` | 50052 | gRPC port |
| `LIBP2P_PORT` | 4001 | P2P discovery port |
| `GATEWAY_URL` | - | Gateway gRPC address |

## Development

```bash
# Run tests
cargo test

# Run with logging
RUST_LOG=debug cargo run -p cyxcloud-gateway

# Format code
cargo fmt

# Lint
cargo clippy
```

## License

MIT License - see [LICENSE](LICENSE) for details.

## Links

- [CyxWiz Main Project](../README.md)
- [Blockchain Programs](../cyxwiz-blockchain/README.md)
- [Solana Explorer (Devnet)](https://explorer.solana.com/?cluster=devnet)
