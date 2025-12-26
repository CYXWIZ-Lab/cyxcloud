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

## Downloads

Pre-built binaries for all platforms:

| Platform | Download |
|----------|----------|
| **Linux x64** | [cyxcloud-v0.1.0-alpha-linux-x64.tar.gz](https://github.com/CYXWIZ-Lab/cyxcloud/releases/download/v0.1.0-alpha/cyxcloud-v0.1.0-alpha-linux-x64.tar.gz) |
| **macOS x64** | [cyxcloud-v0.1.0-alpha-macos-x64.tar.gz](https://github.com/CYXWIZ-Lab/cyxcloud/releases/download/v0.1.0-alpha/cyxcloud-v0.1.0-alpha-macos-x64.tar.gz) |
| **macOS ARM64** | [cyxcloud-v0.1.0-alpha-macos-arm64.tar.gz](https://github.com/CYXWIZ-Lab/cyxcloud/releases/download/v0.1.0-alpha/cyxcloud-v0.1.0-alpha-macos-arm64.tar.gz) |
| **Windows x64** | [cyxcloud-v0.1.0-alpha-windows-x64.zip](https://github.com/CYXWIZ-Lab/cyxcloud/releases/download/v0.1.0-alpha/cyxcloud-v0.1.0-alpha-windows-x64.zip) |

[All Releases](https://github.com/CYXWIZ-Lab/cyxcloud/releases)

**Included binaries:**
- `cyxcloud-gateway` - S3-compatible API server
- `cyxcloud-node` - Storage node daemon
- `cyxcloud` - Command-line client

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

## Arch Linux Setup

Complete guide for running CyxCloud on Arch Linux.

### Install Dependencies

```bash
# Update system
sudo pacman -Syu

# Install build tools
sudo pacman -S base-devel git rustup protobuf

# Install Rust
rustup default stable

# Install Docker (for full stack)
sudo pacman -S docker docker-compose
sudo systemctl enable --now docker
sudo usermod -aG docker $USER
newgrp docker  # or logout/login
```

### Option 1: Run with Docker Compose (Recommended)

```bash
# Clone repository
git clone https://github.com/CYXWIZ-Lab/cyxcloud.git
cd cyxcloud

# Start full stack (PostgreSQL, Redis, Gateway, 3 Nodes, Rebalancer)
docker compose up -d --build

# View logs
docker compose logs -f

# Check health
curl http://localhost:8080/health

# Test upload
echo "Hello CyxCloud!" > test.txt
./target/release/cyxcloud --gateway http://localhost:8080 upload test.txt -b mybucket

# Stop
docker compose down
```

**Services started:**
| Service | Port | Description |
|---------|------|-------------|
| PostgreSQL | 5432 | Metadata storage |
| Redis | 6379 | Caching |
| Gateway | 8080, 50052 | S3 API + gRPC |
| Node 1 | 50061, 4001, 9091 | Storage + P2P + Metrics |
| Node 2 | 50062, 4002, 9092 | Storage + P2P + Metrics |
| Node 3 | 50063, 4003, 9093 | Storage + P2P + Metrics |

### Option 2: Run from Pre-built Binaries

```bash
# Download latest release
wget https://github.com/CYXWIZ-Lab/cyxcloud/releases/download/v0.1.0-alpha/cyxcloud-v0.1.0-alpha-linux-x64.tar.gz
tar -xzf cyxcloud-v0.1.0-alpha-linux-x64.tar.gz
cd cyxcloud-v0.1.0-alpha-linux-x64

# Install PostgreSQL and Redis
sudo pacman -S postgresql redis
sudo systemctl enable --now postgresql redis

# Initialize PostgreSQL
sudo -u postgres createuser -s cyxcloud
sudo -u postgres createdb cyxcloud

# Run gateway
export DATABASE_URL="postgres://cyxcloud@localhost/cyxcloud"
export REDIS_URL="redis://localhost:6379"
./cyxcloud-gateway &

# Run storage node
export NODE_ID="my-node-001"
export STORAGE_PATH="$HOME/.cyxcloud/data"
export CENTRAL_SERVER_ADDR="http://localhost:50052"
mkdir -p $STORAGE_PATH
./cyxcloud-node &

# Test with CLI
./cyxcloud --gateway http://localhost:8080 status
```

### Option 3: Build from Source

```bash
# Clone repository
git clone https://github.com/CYXWIZ-Lab/cyxcloud.git
cd cyxcloud

# Build all components
cargo build --release

# Binaries are in target/release/
ls target/release/cyxcloud*
```

### Run as Systemd Service

**Gateway service** (`/etc/systemd/system/cyxcloud-gateway.service`):
```ini
[Unit]
Description=CyxCloud Gateway
After=network.target postgresql.service redis.service

[Service]
Type=simple
User=cyxcloud
Environment=RUST_LOG=info
Environment=DATABASE_URL=postgres://cyxcloud@localhost/cyxcloud
Environment=REDIS_URL=redis://localhost:6379
ExecStart=/usr/local/bin/cyxcloud-gateway
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

**Storage node service** (`/etc/systemd/system/cyxcloud-node.service`):
```ini
[Unit]
Description=CyxCloud Storage Node
After=network.target cyxcloud-gateway.service

[Service]
Type=simple
User=cyxcloud
Environment=RUST_LOG=info
Environment=NODE_ID=arch-node-001
Environment=STORAGE_PATH=/var/lib/cyxcloud/data
Environment=CENTRAL_SERVER_ADDR=http://localhost:50052
ExecStart=/usr/local/bin/cyxcloud-node
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
# Install binaries
sudo cp target/release/cyxcloud-gateway /usr/local/bin/
sudo cp target/release/cyxcloud-node /usr/local/bin/
sudo cp target/release/cyxcloud /usr/local/bin/

# Create user and directories
sudo useradd -r -s /bin/false cyxcloud
sudo mkdir -p /var/lib/cyxcloud/data
sudo chown -R cyxcloud:cyxcloud /var/lib/cyxcloud

# Enable services
sudo systemctl daemon-reload
sudo systemctl enable --now cyxcloud-gateway
sudo systemctl enable --now cyxcloud-node

# Check status
systemctl status cyxcloud-gateway
systemctl status cyxcloud-node
```

### Firewall Configuration

```bash
# If using ufw
sudo ufw allow 8080/tcp    # S3 API
sudo ufw allow 50052/tcp   # gRPC
sudo ufw allow 4001/tcp    # libp2p

# If using firewalld
sudo firewall-cmd --permanent --add-port=8080/tcp
sudo firewall-cmd --permanent --add-port=50052/tcp
sudo firewall-cmd --permanent --add-port=4001/tcp
sudo firewall-cmd --reload
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
