# CyxCloud - Distributed Storage Platform

CyxCloud is a distributed storage network for ML datasets, providing secure, redundant, and cost-effective storage through community-operated nodes. Designed for the CyxWiz ML ecosystem.

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
│                    ┌──────────────────┐              node registry      │
│                    │   PostgreSQL     │                     │           │
│                    │   + Redis        │◀────────────────────┘           │
│                    └──────────────────┘                                 │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

## Features

- **S3-Compatible API** - Drop-in replacement for Amazon S3
- **Erasure Coding** - Reed-Solomon 10+4 redundancy (survives 4 node failures)
- **Distributed** - Fault-tolerant across multiple community nodes
- **Proof-of-Storage** - Cryptographic verification of stored data
- **Automatic Rebalancing** - Self-healing network
- **Fault Tolerance** - Automatic node lifecycle management with chunk evacuation

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
| **cyxcloud-gateway** | S3 API, request routing | 8080 (HTTP), 50051 (gRPC) |
| **cyxcloud-node** | Storage node, chunk storage | 50052 (gRPC) |
| **cyxcloud-cli** | Command-line client for file operations | - |

## Payments

The Solana integration (on-chain subscriptions, node staking, CYXWIZ token rewards) was removed on 2026-11-04. Storage plans and node rewards move to account credits.

## Quick Start

### Prerequisites

- Rust 1.75+
- PostgreSQL 14+
- Redis 7+

### Build

```bash
cd cyx_cloud

# Build all components
cargo build --release
```

### Run Gateway

```bash
# Set environment variables
export DATABASE_URL="postgres://user:pass@localhost/cyxcloud"
export REDIS_URL="redis://localhost:6379"

# Run gateway
cargo run --release -p cyxcloud-gateway
```

### Run Storage Node

```bash
# Configure node
export NODE_ID="my-node-001"
export STORAGE_PATH="/data/cyxcloud"
export GRPC_PORT=50052

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
| Node 1 | 50061, 9091 | Storage + Metrics |
| Node 2 | 50062, 9092 | Storage + Metrics |
| Node 3 | 50063, 9093 | Storage + Metrics |

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

# If using firewalld
sudo firewall-cmd --permanent --add-port=8080/tcp
sudo firewall-cmd --permanent --add-port=50052/tcp
sudo firewall-cmd --reload
```

## Deploy to Railway (Free Tier)

Deploy CyxCloud Gateway to [Railway](https://railway.app) with one click:

[![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/template/cyxcloud?referralCode=cyxwiz)

### Manual Railway Setup

1. **Create Railway Account**: Sign up at [railway.app](https://railway.app)

2. **Create New Project**: Click "New Project" → "Deploy from GitHub repo"

3. **Connect Repository**: Select `CYXWIZ-Lab/cyxcloud`

4. **Add PostgreSQL**: Click "New" → "Database" → "Add PostgreSQL"

5. **Add Redis**: Click "New" → "Database" → "Add Redis"

6. **Configure Environment Variables**:
   ```
   DATABASE_URL        → ${{Postgres.DATABASE_URL}}
   REDIS_URL           → ${{Redis.REDIS_URL}}
   RUST_LOG            → info
   GATEWAY_HTTP_PORT   → 8080
   GATEWAY_GRPC_PORT   → 50052
   ```

7. **Deploy**: Railway auto-deploys on every push to main

### Railway Endpoints

After deployment, your gateway will be available at:
- **HTTP API**: `https://your-project.up.railway.app`
- **Health Check**: `https://your-project.up.railway.app/health`

## Documentation

- **[Usage Guide](USAGE.md)** - Detailed build, run, and API documentation
- **[Architecture](ARCHITECTURE.md)** - System design and data flow
- **[Vision](VISION.md)** - Project roadmap and goals
- **[Infrastructure](infrastructure.md)** - Deployment and scaling

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

### Fault Tolerance (Gateway)

| Variable | Default | Description |
|----------|---------|-------------|
| `NODE_OFFLINE_THRESHOLD_SECS` | 300 | Mark node offline after 5 min without heartbeat |
| `NODE_DRAIN_THRESHOLD_SECS` | 14400 | Start chunk evacuation after 4 hours offline |
| `NODE_REMOVE_THRESHOLD_SECS` | 604800 | Auto-remove node after 7 days offline |
| `NODE_RECOVERY_QUARANTINE_SECS` | 300 | Quarantine period for reconnecting nodes |
| `NODE_MONITOR_INTERVAL_SECS` | 30 | Node status check interval |

### Storage Node

| Variable | Default | Description |
|----------|---------|-------------|
| `NODE_ID` | - | Unique node identifier |
| `STORAGE_PATH` | ./data | Local storage directory |
| `GRPC_PORT` | 50052 | gRPC port |
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
