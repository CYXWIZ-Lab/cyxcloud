# CyxCloud Use Cases Guide

This guide covers real-world use cases for CyxCloud distributed storage. Whether you're storing files, running ML workloads, or earning tokens as a node operator, this document shows you how.

## Table of Contents

1. [Who is CyxCloud For?](#who-is-cyxcloud-for)
2. [Use Case 1: Personal Cloud Storage](#use-case-1-personal-cloud-storage)
3. [Use Case 2: ML Dataset Storage](#use-case-2-ml-dataset-storage)
4. [Use Case 3: Running a Storage Node (Mining)](#use-case-3-running-a-storage-node-mining)
5. [Use Case 4: Developer Integration](#use-case-4-developer-integration)
6. [Use Case 5: Enterprise Backup](#use-case-5-enterprise-backup)
7. [Web Dashboard Guide](#web-dashboard-guide)
8. [Subscription Management](#subscription-management)
9. [Troubleshooting Common Issues](#troubleshooting-common-issues)

---

## Who is CyxCloud For?

| User Type | Use Case | Benefits |
|-----------|----------|----------|
| **Individuals** | Personal file storage, backups | Distributed, fault-tolerant |
| **ML Engineers** | Dataset storage, model checkpoints | Optimized for large files, CyxWiz integration |
| **Node Operators** | Earn CYXWIZ tokens | Passive income from spare storage |
| **Developers** | S3-compatible storage backend | Drop-in replacement, no vendor lock-in |
| **Enterprises** | Distributed backups, compliance | Geographic redundancy, audit trail |

---

## Use Case 1: Personal Cloud Storage

Store your personal files on a distributed network with end-to-end encryption.

### Step 1: Create Account and Subscribe

1. Visit the CyxWiz web dashboard: `https://app.cyxwiz.com`
2. Create an account or login
3. Navigate to **Drive > Upgrade**
4. Choose a storage plan:

| Plan | Storage | Monthly Price |
|------|---------|---------------|
| Free | 5 GB | 0 CYXWIZ |
| Starter | 100 GB | 10 CYXWIZ |
| Pro | 1 TB | 50 CYXWIZ |
| Enterprise | 10 TB | 200 CYXWIZ |

5. Pay with CYXWIZ tokens from your CyxWallet

### Step 2: Upload Files via Web Dashboard

1. Go to **Drive** in the dashboard
2. Click **Upload** or drag-and-drop files
3. Files are automatically:
   - Split into chunks
   - Erasure-coded (10 data + 4 parity shards)
   - Distributed across 14 storage nodes
4. Monitor upload progress in real-time

### Step 3: Upload Files via CLI

```bash
# Install CLI
cargo install --path cyx_cloud/cyxcloud-cli

# Configure endpoint
export CYXCLOUD_ENDPOINT="https://gateway.cyxcloud.io"
export CYXCLOUD_TOKEN="your-api-token"

# Upload a file
cyxcloud upload ./vacation-photos.zip -b my-files

# Upload entire directory
cyxcloud upload ./documents/ -b my-files -p backups/2025/

# List your files
cyxcloud list my-files -l -H
```

**Example Output:**
```
KEY                                      SIZE        LAST MODIFIED
--------------------------------------------------------------------------------
backups/2025/documents/resume.pdf        245.3 KB    2025-12-23T10:30:00Z
backups/2025/documents/taxes.xlsx        1.2 MB      2025-12-23T10:30:15Z
vacation-photos.zip                      892.5 MB    2025-12-23T10:25:00Z
--------------------------------------------------------------------------------
3 objects, 893.9 MB total
```

### Step 4: Download Files

```bash
# Download single file
cyxcloud download my-files -k vacation-photos.zip -o ./downloads/

# Download entire backup
cyxcloud download my-files --prefix backups/2025/ -o ./restore/
```

### Step 5: Share Files (Coming Soon)

Generate a shareable link with expiration:
```bash
cyxcloud share my-files/vacation-photos.zip --expires 7d
# Returns: https://gateway.cyxcloud.io/share/abc123...
```

---

## Use Case 2: ML Dataset Storage

CyxCloud is optimized for machine learning workloads with large datasets.

### Why CyxCloud for ML?

- **Large File Support**: Optimized for multi-GB datasets
- **Streaming Downloads**: Start training before full download
- **CyxWiz Integration**: Direct dataset loading in CyxWiz Engine
- **Versioning**: Track dataset versions for reproducibility

### Step 1: Organize Your Datasets

```
datasets/
├── mnist/
│   ├── train-images.idx3-ubyte
│   ├── train-labels.idx1-ubyte
│   ├── test-images.idx3-ubyte
│   └── test-labels.idx1-ubyte
├── imagenet/
│   ├── train/
│   └── val/
└── custom-dataset/
    ├── data.csv
    └── images/
```

### Step 2: Upload Dataset

```bash
# Upload MNIST dataset
cyxcloud upload ./datasets/mnist/ -b ml-datasets -p mnist/v1/

# Upload large ImageNet (shows progress)
cyxcloud upload ./datasets/imagenet/ -b ml-datasets -p imagenet/2024/

# Upload with metadata
cyxcloud upload ./datasets/custom-dataset/ \
  -b ml-datasets \
  -p custom/v1/ \
  --metadata '{"version": "1.0", "samples": 50000}'
```

### Step 3: Use in CyxWiz Engine

In CyxWiz Engine, configure the dataset node to load from CyxCloud:

```python
# In your CyxWiz training script
import pycyxwiz

# Configure CyxCloud as data source
dataset = pycyxwiz.Dataset.from_cyxcloud(
    bucket="ml-datasets",
    prefix="mnist/v1/",
    cache_dir="./cache"  # Local cache for faster access
)

# Create data loader
train_loader = pycyxwiz.DataLoader(
    dataset,
    batch_size=64,
    shuffle=True,
    num_workers=4
)

# Train your model
for batch in train_loader:
    # Training loop...
```

### Step 4: Save Model Checkpoints

```bash
# After training, upload checkpoints
cyxcloud upload ./checkpoints/model_epoch_100.pt \
  -b ml-models \
  -p resnet50/v1/

# Upload with auto-versioning
cyxcloud upload ./checkpoints/ \
  -b ml-models \
  -p resnet50/ \
  --version auto
```

### Step 5: Dataset Versioning

Track different versions of your datasets:

```bash
# Upload new version
cyxcloud upload ./datasets/custom-v2/ -b ml-datasets -p custom/v2/

# List all versions
cyxcloud list ml-datasets -p custom/ -l
```

**Output:**
```
KEY                          SIZE        LAST MODIFIED
--------------------------------------------------------------------------------
custom/v1/data.csv           45.2 MB     2025-12-20T14:00:00Z
custom/v1/images/            2.1 GB      2025-12-20T14:05:00Z
custom/v2/data.csv           52.8 MB     2025-12-23T09:00:00Z
custom/v2/images/            2.4 GB      2025-12-23T09:15:00Z
```

---

## Use Case 3: Running a Storage Node (Mining)

Earn CYXWIZ tokens by providing storage to the network.

### Requirements

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| CPU | 2 cores | 4+ cores |
| RAM | 2 GB | 8+ GB |
| Storage | 100 GB SSD | 1+ TB NVMe |
| Network | 100 Mbps | 1 Gbps |
| Uptime | 95%+ | 99%+ |

### Step 1: Install Node Software

```bash
# Clone repository
git clone https://github.com/cyxwiz/cyxcloud.git
cd cyxcloud/cyx_cloud

# Build storage node
cargo build -p cyxcloud-node --release
```

### Step 2: Create Configuration

Create `node-config.toml`:

```toml
[node]
id = "my-mining-node-001"
name = "My Home Server"
region = "us-east"
wallet_address = "YOUR_SOLANA_WALLET_ADDRESS"

[storage]
data_dir = "/data/cyxcloud"
max_capacity_gb = 500
compression = true
cache_size_mb = 1024

[network]
bind_address = "0.0.0.0"
grpc_port = 50051
p2p_port = 4001
public_address = "your-public-ip-or-domain"

[central]
address = "https://gateway.cyxcloud.io:50051"
register = true
heartbeat_interval_secs = 30

[cyxwiz_api]
base_url = "https://api.cyxwiz.com"
register = true

[metrics]
enabled = true
port = 9090
```

### Step 3: Start the Node

```bash
# Run with config file
./target/release/cyxcloud-node --config node-config.toml

# First run will prompt for login
```

**First Run Output:**
```
2025-12-23T10:00:00  INFO  CyxCloud Storage Node starting...

╔══════════════════════════════════════════════════════════════╗
║           CyxWiz Storage Node - Login Required               ║
╚══════════════════════════════════════════════════════════════╝

Please login with your CyxWiz account to register this node.

Email: miner@example.com
Password: ••••••••

Logging in...
✓ Login successful! Welcome, John Doe!

Registering machine...
✓ Machine registered: my-mining-node-001

========================================
  Storage Node Running
  Capacity: 500 GB
  gRPC: 0.0.0.0:50051
  Metrics: http://localhost:9090/metrics
========================================
```

### Step 4: Stake CYXWIZ Tokens

To receive job assignments and earn rewards, stake tokens:

```bash
# Using Solana CLI
solana transfer YOUR_NODE_STAKE_PDA 500 --allow-unfunded-recipient

# Or via CyxWiz web dashboard:
# 1. Go to Dashboard > Wallet
# 2. Click "Stake for Storage Node"
# 3. Enter amount (minimum 500 CYXWIZ)
# 4. Confirm transaction
```

### Step 5: Monitor Earnings

Check your node status and earnings:

```bash
# View metrics
curl http://localhost:9090/metrics | grep cyxcloud

# Key metrics:
# cyxcloud_storage_bytes_used      - Current storage usage
# cyxcloud_chunks_total            - Number of chunks stored
# cyxcloud_bandwidth_in_bytes      - Incoming bandwidth
# cyxcloud_bandwidth_out_bytes     - Outgoing bandwidth
# cyxcloud_heartbeat_success_total - Successful heartbeats
```

**Via Web Dashboard:**
1. Go to **Dashboard > Machines**
2. View your node's:
   - Online status
   - Storage used/available
   - Earnings (CYXWIZ/day)
   - Uptime percentage

### Reward Calculation

Rewards are distributed weekly based on:

```
Node Score = (Storage_Weight × 0.4) + (Uptime_Weight × 0.3) +
             (Bandwidth_Weight × 0.2) + (Reputation_Weight × 0.1)

Weekly Reward = (Node_Score / Total_Network_Score) × Weekly_Pool × 0.85
```

**Example:**
- You provide 500 GB storage with 99% uptime
- Network has 100 TB total storage
- Weekly pool: 10,000 CYXWIZ
- Your share: ~4.25 CYXWIZ/week (before stake weight bonus)

### Step 6: Run as a Service

**Linux (systemd):**

Create `/etc/systemd/system/cyxcloud-node.service`:
```ini
[Unit]
Description=CyxCloud Storage Node
After=network.target

[Service]
Type=simple
User=cyxcloud
ExecStart=/opt/cyxcloud/cyxcloud-node --config /etc/cyxcloud/node.toml
Restart=always
RestartSec=10
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable cyxcloud-node
sudo systemctl start cyxcloud-node
sudo journalctl -u cyxcloud-node -f
```

**Windows (NSSM):**
```powershell
nssm install CyxCloudNode "C:\CyxCloud\cyxcloud-node.exe"
nssm set CyxCloudNode AppParameters "--config C:\CyxCloud\node.toml"
nssm start CyxCloudNode
```

---

## Use Case 4: Developer Integration

Integrate CyxCloud into your applications using the S3-compatible API.

### S3 SDK Compatibility

CyxCloud works with any S3-compatible SDK:

**Python (boto3):**
```python
import boto3

s3 = boto3.client(
    's3',
    endpoint_url='https://gateway.cyxcloud.io',
    aws_access_key_id='your-api-key',
    aws_secret_access_key='your-api-secret'
)

# Upload file
s3.upload_file('local-file.txt', 'my-bucket', 'remote-file.txt')

# Download file
s3.download_file('my-bucket', 'remote-file.txt', 'downloaded.txt')

# List objects
response = s3.list_objects_v2(Bucket='my-bucket', Prefix='data/')
for obj in response.get('Contents', []):
    print(f"{obj['Key']}: {obj['Size']} bytes")
```

**JavaScript (AWS SDK v3):**
```javascript
import { S3Client, PutObjectCommand, GetObjectCommand } from '@aws-sdk/client-s3';

const s3 = new S3Client({
  endpoint: 'https://gateway.cyxcloud.io',
  region: 'us-east-1',
  credentials: {
    accessKeyId: 'your-api-key',
    secretAccessKey: 'your-api-secret'
  }
});

// Upload
await s3.send(new PutObjectCommand({
  Bucket: 'my-bucket',
  Key: 'data/file.json',
  Body: JSON.stringify({ hello: 'world' }),
  ContentType: 'application/json'
}));

// Download
const response = await s3.send(new GetObjectCommand({
  Bucket: 'my-bucket',
  Key: 'data/file.json'
}));
const data = await response.Body.transformToString();
```

**Go:**
```go
package main

import (
    "github.com/aws/aws-sdk-go/aws"
    "github.com/aws/aws-sdk-go/aws/credentials"
    "github.com/aws/aws-sdk-go/aws/session"
    "github.com/aws/aws-sdk-go/service/s3"
)

func main() {
    sess := session.Must(session.NewSession(&aws.Config{
        Endpoint:         aws.String("https://gateway.cyxcloud.io"),
        Region:           aws.String("us-east-1"),
        Credentials:      credentials.NewStaticCredentials("key", "secret", ""),
        S3ForcePathStyle: aws.Bool(true),
    }))

    client := s3.New(sess)

    // Upload
    _, err := client.PutObject(&s3.PutObjectInput{
        Bucket: aws.String("my-bucket"),
        Key:    aws.String("data/file.txt"),
        Body:   strings.NewReader("Hello, CyxCloud!"),
    })
}
```

### REST API Direct Usage

```bash
# Upload
curl -X PUT https://gateway.cyxcloud.io/s3/my-bucket/file.txt \
  -H "Authorization: Bearer YOUR_API_TOKEN" \
  -H "Content-Type: text/plain" \
  -d "Hello, World!"

# Download
curl https://gateway.cyxcloud.io/s3/my-bucket/file.txt \
  -H "Authorization: Bearer YOUR_API_TOKEN" \
  -o downloaded.txt

# List
curl "https://gateway.cyxcloud.io/s3/my-bucket?list-type=2" \
  -H "Authorization: Bearer YOUR_API_TOKEN"

# Delete
curl -X DELETE https://gateway.cyxcloud.io/s3/my-bucket/file.txt \
  -H "Authorization: Bearer YOUR_API_TOKEN"
```

### WebSocket Events

Subscribe to real-time events for upload progress and file changes:

```javascript
const ws = new WebSocket('wss://gateway.cyxcloud.io/ws');

ws.onopen = () => {
  // Authenticate
  ws.send(JSON.stringify({
    type: 'auth',
    token: 'YOUR_API_TOKEN'
  }));

  // Subscribe to events
  ws.send(JSON.stringify({
    type: 'subscribe',
    topics: ['file.created', 'file.deleted', 'upload.progress']
  }));
};

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);

  switch (data.type) {
    case 'file.created':
      console.log(`New file: ${data.bucket}/${data.key}`);
      break;
    case 'upload.progress':
      console.log(`Upload ${data.uploadId}: ${data.progress}%`);
      break;
  }
};
```

### Generating API Keys

1. Go to **Dashboard > Settings > API Keys**
2. Click **Create New Key**
3. Set permissions:
   - `read` - Download files
   - `write` - Upload files
   - `delete` - Delete files
   - `admin` - Manage buckets
4. Copy the key (shown only once)

---

## Use Case 5: Enterprise Backup

Set up automated backups with geographic redundancy.

### Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  Primary Site   │────▶│  CyxCloud GW    │────▶│  Storage Nodes  │
│  (Your Servers) │     │  (S3 API)       │     │  (Multi-region) │
└─────────────────┘     └─────────────────┘     └─────────────────┘
        │                                               │
        │              ┌─────────────────┐             │
        └─────────────▶│  Solana Chain   │◀────────────┘
                       │  (Audit Trail)  │
                       └─────────────────┘
```

### Step 1: Configure Backup Tool

**Using rclone:**

Create `~/.config/rclone/rclone.conf`:
```ini
[cyxcloud]
type = s3
provider = Other
endpoint = https://gateway.cyxcloud.io
access_key_id = YOUR_API_KEY
secret_access_key = YOUR_API_SECRET
```

```bash
# Sync directory to CyxCloud
rclone sync /data/important/ cyxcloud:backups/daily/

# With encryption
rclone sync /data/important/ cyxcloud:backups/daily/ --crypt-password YOUR_PASSWORD
```

**Using restic:**

```bash
# Initialize repository
export RESTIC_REPOSITORY="s3:https://gateway.cyxcloud.io/backups"
export AWS_ACCESS_KEY_ID="YOUR_API_KEY"
export AWS_SECRET_ACCESS_KEY="YOUR_API_SECRET"

restic init

# Create backup
restic backup /data/important/

# List snapshots
restic snapshots

# Restore
restic restore latest --target /restore/
```

### Step 2: Automate with Cron

```bash
# Daily backup at 2 AM
0 2 * * * /usr/local/bin/rclone sync /data/ cyxcloud:backups/$(date +\%Y-\%m-\%d)/ >> /var/log/backup.log 2>&1
```

### Step 3: Monitor via Dashboard

1. **Dashboard > Drive** - View storage usage
2. **Dashboard > Billing** - Track costs
3. **Dashboard > Activity** - Audit log of all operations

### Compliance Features

- **Immutable Backups**: Enable versioning to prevent deletion
- **Audit Trail**: All operations recorded on Solana blockchain
- **Geographic Distribution**: Data replicated across regions
- **Encryption**: Client-side encryption supported

---

## Web Dashboard Guide

The CyxWiz web dashboard provides a user-friendly interface for CyxCloud.

### Dashboard Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  CyxWiz Dashboard                                    [User ▼]   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐        │
│  │  Drive   │  │ Machines │  │  Wallet  │  │ Settings │        │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘        │
│                                                                 │
│  Storage Usage                    Quick Actions                 │
│  ┌─────────────────────────┐     ┌──────────────────────┐      │
│  │ ████████░░░░ 45.2 GB    │     │ [Upload Files]       │      │
│  │ of 100 GB (Starter)     │     │ [Create Folder]      │      │
│  └─────────────────────────┘     │ [Upgrade Plan]       │      │
│                                  └──────────────────────┘      │
│  Recent Files                                                  │
│  ┌─────────────────────────────────────────────────────┐      │
│  │ 📄 report.pdf          2.3 MB    Dec 23, 2025      │      │
│  │ 📁 datasets/           1.2 GB    Dec 22, 2025      │      │
│  │ 🖼 photo.jpg           4.5 MB    Dec 21, 2025      │      │
│  └─────────────────────────────────────────────────────┘      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Drive Page

**Features:**
- File browser with folder navigation
- Drag-and-drop upload
- Multi-file selection
- Context menu (download, delete, share)
- Search and filter
- Grid/List view toggle

**Upload Flow:**
1. Click "Upload" or drag files
2. Progress bar shows upload status
3. Files appear in list when complete
4. Click file to view details or download

### Machines Page (For Node Operators)

View and manage your storage nodes:

```
┌─────────────────────────────────────────────────────────────────┐
│  My Machines                                    [Register New]  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ ● my-mining-node-001                        [Online]    │   │
│  │   Storage: 234.5 GB / 500 GB                            │   │
│  │   Uptime: 99.8%                                         │   │
│  │   Earnings: 12.5 CYXWIZ this week                       │   │
│  │   Last heartbeat: 30 seconds ago                        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ ○ backup-node-002                           [Offline]   │   │
│  │   Storage: 0 GB / 1000 GB                               │   │
│  │   Last seen: 2 hours ago                                │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Wallet Page

Manage your CYXWIZ tokens and Solana wallet:

- View SOL and CYXWIZ balances
- Send/receive tokens
- View transaction history
- Stake tokens for storage nodes
- Manage subscriptions

---

## Subscription Management

### Viewing Current Plan

**Dashboard > Drive > Storage Usage**

Shows:
- Current plan (Free/Starter/Pro/Enterprise)
- Storage used vs quota
- Bandwidth used this period
- Renewal date

### Upgrading Plan

1. Go to **Drive > Upgrade**
2. Select payment method:
   - **CYXWIZ Tokens** (Recommended) - Pay from CyxWallet
   - **Credit Card** (Coming soon)
3. Choose plan and billing period (Monthly/Yearly)
4. Confirm transaction

### Downgrading Plan

1. Ensure storage used is below new plan's limit
2. Go to **Settings > Subscription**
3. Click "Change Plan"
4. Select lower tier
5. Confirm (takes effect at end of billing period)

### Cancelling Subscription

1. Go to **Settings > Subscription**
2. Click "Cancel Subscription"
3. Data is preserved for 30 days
4. After 30 days, data moves to cold storage
5. After 90 days, data is deleted

### Transaction History

View all blockchain transactions:

**Dashboard > Wallet > Transactions**

```
┌─────────────────────────────────────────────────────────────────┐
│  Transaction History                                            │
├─────────────────────────────────────────────────────────────────┤
│  Date          Type              Amount      Status    Tx       │
├─────────────────────────────────────────────────────────────────┤
│  Dec 23, 2025  Subscription      -10 CYXWIZ  ✓ Success [View]   │
│  Dec 22, 2025  Node Reward       +4.25 CYXWIZ ✓ Success [View]  │
│  Dec 15, 2025  Stake             -500 CYXWIZ ✓ Success [View]   │
│  Dec 01, 2025  Subscription      -10 CYXWIZ  ✓ Success [View]   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Troubleshooting Common Issues

### "Failed to upload file"

**Cause:** Exceeded storage quota

**Solution:**
1. Check quota: **Drive > Storage Usage**
2. Delete unused files or upgrade plan
3. Retry upload

### "Connection refused" when running node

**Cause:** Gateway not reachable

**Solution:**
```bash
# Check gateway status
curl https://gateway.cyxcloud.io/health

# Verify network connectivity
ping gateway.cyxcloud.io

# Check firewall allows outbound 443, 50051
```

### "Invalid wallet address" error

**Cause:** Wallet address format incorrect

**Solution:**
- Ensure using Solana base58 address (44 characters)
- Example: `4Y5HWB9W9SELq3Yoyf7mK7KF5kTbuaGxd2BMvn3AyAG8`

### Node shows "Offline" in dashboard

**Cause:** Heartbeat not reaching gateway

**Solution:**
1. Check node logs: `journalctl -u cyxcloud-node -f`
2. Verify `central.address` in config is correct
3. Ensure port 50051 is open for outbound connections
4. Check JWT token hasn't expired (re-login if needed)

### "Insufficient funds" when subscribing

**Cause:** Not enough CYXWIZ in CyxWallet

**Solution:**
1. Check balance: **Dashboard > Wallet**
2. Acquire CYXWIZ tokens from exchange
3. Or earn by running a storage node

### Slow upload speeds

**Cause:** Network congestion or distant nodes

**Solution:**
1. Check your internet upload speed
2. Try during off-peak hours
3. For large files, use CLI instead of web (supports resumable uploads)

### "Chunk verification failed"

**Cause:** Data corruption during transfer

**Solution:**
- This is automatically retried
- If persistent, check disk health on your node
- Contact support if issue continues

---

## Getting Help

- **Documentation**: https://docs.cyxwiz.com
- **Discord**: https://discord.gg/cyxwiz
- **GitHub Issues**: https://github.com/cyxwiz/cyxcloud/issues
- **Email Support**: support@cyxwiz.com

---

## Quick Reference

### CLI Commands

| Command | Description |
|---------|-------------|
| `cyxcloud upload <path> -b <bucket>` | Upload file or directory |
| `cyxcloud download <bucket> -k <key>` | Download file |
| `cyxcloud list <bucket>` | List objects |
| `cyxcloud delete <bucket> -k <key>` | Delete object |
| `cyxcloud status` | Check gateway status |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `CYXCLOUD_ENDPOINT` | Gateway URL |
| `CYXCLOUD_TOKEN` | API token |
| `RUST_LOG` | Log level (debug/info/warn/error) |

### API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/s3/<bucket>/<key>` | PUT | Upload object |
| `/s3/<bucket>/<key>` | GET | Download object |
| `/s3/<bucket>/<key>` | DELETE | Delete object |
| `/s3/<bucket>?list-type=2` | GET | List objects |
| `/health` | GET | Health check |
| `/ws` | WebSocket | Real-time events |

### Storage Plans

| Plan | Storage | Monthly | Yearly (20% off) |
|------|---------|---------|------------------|
| Free | 5 GB | 0 | - |
| Starter | 100 GB | 10 CYXWIZ | 96 CYXWIZ |
| Pro | 1 TB | 50 CYXWIZ | 480 CYXWIZ |
| Enterprise | 10 TB | 200 CYXWIZ | 1,920 CYXWIZ |
