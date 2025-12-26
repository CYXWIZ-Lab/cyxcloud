# CyxCloud Technical Architecture

This document provides a complete technical specification of the CyxCloud decentralized storage platform, including all APIs, protocols, data flows, and component designs.

## Table of Contents

1. [System Overview](#system-overview)
2. [Component Architecture](#component-architecture)
3. [User Flow & Website Integration](#user-flow--website-integration)
4. [API Specifications](#api-specifications)
   - [S3-Compatible REST API](#s3-compatible-rest-api)
   - [gRPC ChunkService API](#grpc-chunkservice-api)
   - [WebSocket Events API](#websocket-events-api)
   - [Authentication API](#authentication-api)
5. [Data Structures](#data-structures)
6. [Protocol Definitions](#protocol-definitions)
7. [Storage Architecture](#storage-architecture)
8. [Network Architecture](#network-architecture)
9. [Security Architecture](#security-architecture)
10. [Token Economics & Payments](#token-economics--payments)
11. [CyxHub Architecture](#cyxhub-architecture)
12. [Error Handling](#error-handling)
13. [Configuration Reference](#configuration-reference)

---

## System Overview

CyxCloud is a decentralized storage platform that integrates with the CyxWiz ecosystem. It provides private cloud storage for users, a marketplace for ML datasets, and a distributed network of storage providers.

### High-Level Architecture

```
+------------------------------------------------------------------------------+
|                             CyxWiz Ecosystem                                   |
+------------------------------------------------------------------------------+
|                                                                                |
|  +------------------+     +------------------+     +------------------+        |
|  |   CyxWiz         |     |    CyxCloud      |     |   CyxWiz         |        |
|  |   Website        |     |    Gateway       |     |   Engine         |        |
|  |   (React/Next)   |     |    (Rust/Axum)   |     |   (C++/ImGui)    |        |
|  +--------+---------+     +--------+---------+     +--------+---------+        |
|           |                        |                        |                  |
|           |   Login/Signup         |   S3 API               |   S3 API        |
|           |   Wallet               |   gRPC                 |   gRPC          |
|           v                        v                        v                  |
|  +----------------------------+    |    +----------------------------+        |
|  |      User Account          |    |    |      CLI Tool              |        |
|  |      CYXWIZ Wallet         |    |    |      (cyxcloud)            |        |
|  |      Storage Plans         |    |    +----------------------------+        |
|  +----------------------------+    |                                           |
|                                    v                                           |
|  +----------------------------------------------------------------------+     |
|  |                         Storage Layer                                  |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  |  | Storage Node 1 |  | Storage Node 2 |  | Storage Node N |           |     |
|  |  | (Miner)        |  | (Miner)        |  | (Volunteer)    |           |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  +----------------------------------------------------------------------+     |
|                                    |                                           |
|                                    v                                           |
|  +----------------------------------------------------------------------+     |
|  |                         Metadata Layer                                 |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  |  |  PostgreSQL    |  |     Redis      |  |   Meilisearch  |           |     |
|  |  |  (Metadata)    |  |    (Cache)     |  |   (Search)     |           |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  +----------------------------------------------------------------------+     |
|                                    |                                           |
|  +----------------------------------------------------------------------+     |
|  |                         Blockchain Layer                               |     |
|  |  +----------------------------+  +----------------------------+       |     |
|  |  |        Solana              |  |     CYXWIZ Token           |       |     |
|  |  |   (Payments/Escrow)        |  |    (SPL Token)             |       |     |
|  |  +----------------------------+  +----------------------------+       |     |
|  +----------------------------------------------------------------------+     |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Key Design Principles

1. **Gateway as Central Server**: The Gateway is the single entry point for all client requests
2. **Decentralized Storage**: Data is distributed across multiple nodes using erasure coding
3. **Token-Based Payments**: CYXWIZ tokens for storage fees and miner rewards
4. **Private by Default**: User data is encrypted and private unless explicitly shared
5. **Hybrid Networking**: gRPC for data transfer, libp2p for peer discovery

---

## Component Architecture

### Gateway (Central Server)

The Gateway is the core orchestrator that handles all client requests.

```
+------------------------------------------------------------------------------+
|                              Gateway Architecture                              |
+------------------------------------------------------------------------------+
|                                                                                |
|  +------------------+     +------------------+     +------------------+        |
|  |   HTTP Server    |     |   gRPC Server    |     |  WebSocket       |        |
|  |   (Axum)         |     |   (Tonic)        |     |  Server          |        |
|  +--------+---------+     +--------+---------+     +--------+---------+        |
|           |                        |                        |                  |
|           +------------------------+------------------------+                  |
|                                    |                                           |
|                                    v                                           |
|  +----------------------------------------------------------------------+     |
|  |                         Request Router                                 |     |
|  +----------------------------------------------------------------------+     |
|           |                        |                        |                  |
|           v                        v                        v                  |
|  +------------------+     +------------------+     +------------------+        |
|  |  S3 Handler      |     | Chunk Handler    |     | Event Handler    |        |
|  |  (REST API)      |     | (gRPC)           |     | (WebSocket)      |        |
|  +--------+---------+     +--------+---------+     +--------+---------+        |
|           |                        |                        |                  |
|           +------------------------+------------------------+                  |
|                                    |                                           |
|                                    v                                           |
|  +----------------------------------------------------------------------+     |
|  |                      Core Services                                     |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  |  | Storage        |  | Metadata       |  | Auth           |           |     |
|  |  | Coordinator    |  | Service        |  | Service        |           |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  |  | Erasure        |  | Node           |  | Payment        |           |     |
|  |  | Encoder        |  | Selector       |  | Processor      |           |     |
|  |  +----------------+  +----------------+  +----------------+           |     |
|  +----------------------------------------------------------------------+     |
|                                                                                |
+------------------------------------------------------------------------------+
```

**Responsibilities:**
- Accept HTTP/S3 requests from clients (CLI, SDK, Engine)
- Authenticate users via JWT tokens
- Coordinate chunk distribution to storage nodes
- Maintain metadata in PostgreSQL
- Stream real-time events via WebSocket
- Process CYXWIZ token payments

### Storage Node

Storage nodes store and serve chunks of data.

```
+------------------------------------------------------------------------------+
|                           Storage Node Architecture                            |
+------------------------------------------------------------------------------+
|                                                                                |
|  +------------------+     +------------------+     +------------------+        |
|  |   gRPC Server    |     |  libp2p Swarm    |     |  Health          |        |
|  |   (ChunkService) |     |  (Discovery)     |     |  Reporter        |        |
|  +--------+---------+     +--------+---------+     +--------+---------+        |
|           |                        |                        |                  |
|           +------------------------+------------------------+                  |
|                                    |                                           |
|                                    v                                           |
|  +----------------------------------------------------------------------+     |
|  |                         Chunk Manager                                  |     |
|  +----------------------------------------------------------------------+     |
|           |                        |                        |                  |
|           v                        v                        v                  |
|  +------------------+     +------------------+     +------------------+        |
|  |  Storage Backend |     | Integrity        |     | Metrics          |        |
|  |  (RocksDB)       |     | Verifier         |     | Collector        |        |
|  +------------------+     +------------------+     +------------------+        |
|                                                                                |
+------------------------------------------------------------------------------+
```

**Node Types:**

| Type | Description | Payment | Requirements |
|------|-------------|---------|--------------|
| **Miner** | Commercial storage provider | Earns CYXWIZ tokens | Stake required, SLA |
| **Volunteer** | Free storage contributor | Reputation points | No stake, best effort |
| **Enterprise** | Dedicated private nodes | Fixed contract | Private network |

---

## User Flow & Website Integration

### CyxWiz Website Integration

The CyxWiz website (cyxwiz.com) serves as the primary user interface for account management and storage subscriptions.

```
+------------------------------------------------------------------------------+
|                           Website User Flow                                    |
+------------------------------------------------------------------------------+
|                                                                                |
|  1. SIGNUP/LOGIN                                                              |
|  +------------------------------------------------------------------------+   |
|  |  cyxwiz.com                                                             |   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  |  | Email/Password   | OR  | Wallet Connect   | OR  | Social OAuth     | |   |
|  |  | Signup           |     | (Phantom/Solflare|     | (Google/GitHub)  | |   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|                                    v                                           |
|  2. WALLET SETUP                                                              |
|  +------------------------------------------------------------------------+   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  |  | Generate New     |     | Import Existing  |     | View Balance     | |   |
|  |  | Solana Wallet    |     | Wallet           |     | Buy CYXWIZ       | |   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|                                    v                                           |
|  3. STORAGE SELECTION (Drive Storage Nav)                                     |
|  +------------------------------------------------------------------------+   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  |  | Basic 10GB       |     | Pro 100GB        |     | Enterprise       | |   |
|  |  | 5 CYXWIZ/mo      |     | 40 CYXWIZ/mo     |     | Custom Quote     | |   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  |                                                                         |   |
|  |  [x] Monthly    [ ] Yearly (20% discount)                              |   |
|  |                                                                         |   |
|  |  [Subscribe with CYXWIZ]                                               |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|                                    v                                           |
|  4. GATEWAY ALLOCATION                                                        |
|  +------------------------------------------------------------------------+   |
|  |  Gateway receives subscription:                                         |   |
|  |  - Creates user bucket (private)                                        |   |
|  |  - Allocates storage quota                                              |   |
|  |  - Generates API credentials                                            |   |
|  |  - Returns endpoint URLs                                                |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|                                    v                                           |
|  5. DOWNLOADS                                                                 |
|  +------------------------------------------------------------------------+   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  |  | CLI Tool         |     | Node Software    |     | SDK Docs         | |   |
|  |  | (Windows/Mac/    |     | (For Miners)     |     | (Python/Rust/    | |   |
|  |  |  Linux)          |     |                  |     |  JavaScript)     | |   |
|  |  +------------------+     +------------------+     +------------------+ |   |
|  +------------------------------------------------------------------------+   |
|                                                                                |
+------------------------------------------------------------------------------+
```

### CLI Usage Flow

```
+------------------------------------------------------------------------------+
|                              CLI Usage Flow                                    |
+------------------------------------------------------------------------------+
|                                                                                |
|  # 1. Configure CLI with credentials                                          |
|  $ cyxcloud configure                                                         |
|  Gateway URL: https://gateway.cyxwiz.com                                      |
|  API Key: ****-****-****-****                                                 |
|  Configuration saved to ~/.cyxcloud/config.toml                               |
|                                                                                |
|  # 2. Upload to private drive                                                 |
|  $ cyxcloud upload ./my-dataset.tar.gz -b my-private-bucket                   |
|  Uploading: my-dataset.tar.gz (1.5 GB)                                        |
|  [=====================================] 100% (45 MB/s)                        |
|  Uploaded: cyx://my-private-bucket/my-dataset.tar.gz                          |
|                                                                                |
|  # 3. Link data URI in training config                                        |
|  # In CyxWiz Engine node editor:                                              |
|  # DataInput Node -> URI: cyx://my-private-bucket/my-dataset.tar.gz           |
|                                                                                |
|  # 4. Download from private drive                                             |
|  $ cyxcloud download my-private-bucket -k my-dataset.tar.gz -o ./local/       |
|  Downloading: my-dataset.tar.gz                                               |
|  [=====================================] 100%                                  |
|  Saved to: ./local/my-dataset.tar.gz                                          |
|                                                                                |
|  # 5. Check storage usage                                                     |
|  $ cyxcloud status -b my-private-bucket                                       |
|  Bucket: my-private-bucket (Private)                                          |
|  Used: 1.5 GB / 10 GB (15%)                                                   |
|  Objects: 1                                                                   |
|  Subscription: Basic (5 CYXWIZ/mo, renews Jan 15)                             |
|                                                                                |
+------------------------------------------------------------------------------+
```

---

## API Specifications

### S3-Compatible REST API

The Gateway exposes an S3-compatible REST API for object storage operations.

#### Base URL
```
https://gateway.cyxwiz.com/s3
```

#### Authentication

All requests require authentication via:
- **Bearer Token**: `Authorization: Bearer <jwt-token>`
- **API Key**: `X-API-Key: <api-key>`
- **S3 Signature**: AWS Signature V4 (for S3 client compatibility)

#### Endpoints

##### PUT /s3/{bucket}
Create a new bucket.

**Request:**
```http
PUT /s3/my-bucket HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
Content-Type: application/json

{
  "visibility": "private",
  "erasure_config": {
    "data_shards": 8,
    "parity_shards": 6
  }
}
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "bucket": "my-bucket",
  "created_at": "2024-01-15T10:30:00Z",
  "visibility": "private",
  "endpoint": "https://gateway.cyxwiz.com/s3/my-bucket"
}
```

##### PUT /s3/{bucket}/{key}
Upload an object.

**Request:**
```http
PUT /s3/my-bucket/data/file.txt HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
Content-Type: text/plain
Content-Length: 1024

<file content>
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "key": "data/file.txt",
  "etag": "\"a1b2c3d4e5f6...\"",
  "size": 1024,
  "version_id": "v1"
}
```

##### GET /s3/{bucket}/{key}
Download an object.

**Request:**
```http
GET /s3/my-bucket/data/file.txt HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
Range: bytes=0-999  # Optional range request
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: text/plain
Content-Length: 1024
ETag: "a1b2c3d4e5f6..."
Last-Modified: Mon, 15 Jan 2024 10:30:00 GMT

<file content>
```

##### HEAD /s3/{bucket}/{key}
Get object metadata.

**Request:**
```http
HEAD /s3/my-bucket/data/file.txt HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: text/plain
Content-Length: 1024
ETag: "a1b2c3d4e5f6..."
Last-Modified: Mon, 15 Jan 2024 10:30:00 GMT
X-Cyx-Visibility: private
X-Cyx-Erasure-Config: 8+6
```

##### DELETE /s3/{bucket}/{key}
Delete an object.

**Request:**
```http
DELETE /s3/my-bucket/data/file.txt HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
```

**Response:**
```http
HTTP/1.1 204 No Content
```

##### GET /s3/{bucket}?list-type=2
List objects in a bucket.

**Request:**
```http
GET /s3/my-bucket?list-type=2&prefix=data/&max-keys=100 HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
```

**Response:**
```http
HTTP/1.1 200 OK
Content-Type: application/xml

<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>my-bucket</Name>
  <Prefix>data/</Prefix>
  <MaxKeys>100</MaxKeys>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>data/file1.txt</Key>
    <LastModified>2024-01-15T10:30:00Z</LastModified>
    <ETag>"a1b2c3d4..."</ETag>
    <Size>1024</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <Contents>
    <Key>data/file2.txt</Key>
    <LastModified>2024-01-15T11:00:00Z</LastModified>
    <ETag>"e5f6g7h8..."</ETag>
    <Size>2048</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
</ListBucketResult>
```

##### DELETE /s3/{bucket}
Delete a bucket (must be empty).

**Request:**
```http
DELETE /s3/my-bucket HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
```

**Response:**
```http
HTTP/1.1 204 No Content
```

#### Error Responses

```json
{
  "error": {
    "code": "NoSuchBucket",
    "message": "The specified bucket does not exist",
    "resource": "/s3/nonexistent-bucket",
    "request_id": "req-123456"
  }
}
```

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `NoSuchBucket` | 404 | Bucket does not exist |
| `NoSuchKey` | 404 | Object does not exist |
| `BucketAlreadyExists` | 409 | Bucket name taken |
| `AccessDenied` | 403 | Permission denied |
| `InvalidRequest` | 400 | Malformed request |
| `InsufficientStorage` | 507 | Quota exceeded |
| `InternalError` | 500 | Server error |

---

### gRPC ChunkService API

The gRPC API is used for direct chunk operations between Gateway and Storage Nodes.

#### Protocol Definition

```protobuf
// cyxcloud-protocol/proto/chunk.proto

syntax = "proto3";
package cyxcloud.chunk;

service ChunkService {
  // Store a single chunk
  rpc StoreChunk(StoreChunkRequest) returns (StoreChunkResponse);

  // Retrieve a single chunk
  rpc GetChunk(GetChunkRequest) returns (GetChunkResponse);

  // Delete a chunk
  rpc DeleteChunk(DeleteChunkRequest) returns (DeleteChunkResponse);

  // Stream multiple chunks (upload)
  rpc StreamChunks(stream ChunkData) returns (StreamChunksResponse);

  // Stream multiple chunks (download)
  rpc FetchChunks(FetchChunksRequest) returns (stream ChunkData);

  // Verify chunk integrity
  rpc VerifyChunk(VerifyChunkRequest) returns (VerifyChunkResponse);

  // Health check
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}

message ChunkId {
  bytes hash = 1;  // Blake3 hash (32 bytes)
}

message StoreChunkRequest {
  ChunkId chunk_id = 1;
  bytes data = 2;
  ChunkMetadata metadata = 3;
}

message StoreChunkResponse {
  bool success = 1;
  string error_message = 2;
  uint64 bytes_written = 3;
}

message GetChunkRequest {
  ChunkId chunk_id = 1;
}

message GetChunkResponse {
  bool found = 1;
  bytes data = 2;
  ChunkMetadata metadata = 3;
}

message DeleteChunkRequest {
  ChunkId chunk_id = 1;
}

message DeleteChunkResponse {
  bool success = 1;
  string error_message = 2;
}

message ChunkData {
  ChunkId chunk_id = 1;
  bytes data = 2;
  uint32 shard_index = 3;
}

message StreamChunksResponse {
  uint32 chunks_stored = 1;
  uint64 bytes_stored = 2;
  repeated ChunkId failed_chunks = 3;
}

message FetchChunksRequest {
  repeated ChunkId chunk_ids = 1;
}

message VerifyChunkRequest {
  ChunkId chunk_id = 1;
  bytes expected_hash = 2;  // Optional: verify against specific hash
}

message VerifyChunkResponse {
  bool valid = 1;
  bytes actual_hash = 2;
  string error_message = 3;
}

message ChunkMetadata {
  uint64 original_size = 1;
  uint64 stored_size = 2;
  bool encrypted = 3;
  bool compressed = 4;
  int64 created_at = 5;  // Unix timestamp
  string file_id = 6;
  uint32 chunk_index = 7;
}

message HealthCheckRequest {}

message HealthCheckResponse {
  bool healthy = 1;
  NodeStatus status = 2;
}

message NodeStatus {
  string node_id = 1;
  uint64 capacity_bytes = 2;
  uint64 used_bytes = 3;
  uint64 chunk_count = 4;
  double cpu_usage = 5;
  double memory_usage = 6;
  uint64 uptime_seconds = 7;
}
```

#### Usage Example (Rust)

```rust
use cyxcloud_protocol::chunk::{
    chunk_service_client::ChunkServiceClient,
    StoreChunkRequest, ChunkId, ChunkMetadata,
};

async fn store_chunk(
    client: &mut ChunkServiceClient<Channel>,
    chunk_id: [u8; 32],
    data: Vec<u8>,
) -> Result<()> {
    let request = StoreChunkRequest {
        chunk_id: Some(ChunkId { hash: chunk_id.to_vec() }),
        data,
        metadata: Some(ChunkMetadata {
            original_size: data.len() as u64,
            stored_size: data.len() as u64,
            encrypted: false,
            compressed: false,
            created_at: chrono::Utc::now().timestamp(),
            file_id: "file-123".to_string(),
            chunk_index: 0,
        }),
    };

    let response = client.store_chunk(request).await?;
    println!("Stored {} bytes", response.get_ref().bytes_written);
    Ok(())
}
```

---

### WebSocket Events API

Real-time event streaming for monitoring and notifications.

#### Connection

```javascript
const ws = new WebSocket('wss://gateway.cyxwiz.com/ws');

// Authenticate
ws.onopen = () => {
    ws.send(JSON.stringify({
        type: 'auth',
        token: 'jwt-token-here'
    }));
};
```

#### Event Types

##### File Events

```json
{
  "type": "file.created",
  "timestamp": "2024-01-15T10:30:00Z",
  "data": {
    "bucket": "my-bucket",
    "key": "data/file.txt",
    "size": 1024,
    "content_type": "text/plain",
    "etag": "a1b2c3d4..."
  }
}
```

```json
{
  "type": "file.deleted",
  "timestamp": "2024-01-15T10:35:00Z",
  "data": {
    "bucket": "my-bucket",
    "key": "data/file.txt"
  }
}
```

##### Upload Progress

```json
{
  "type": "upload.progress",
  "timestamp": "2024-01-15T10:30:05Z",
  "data": {
    "upload_id": "upload-123",
    "bucket": "my-bucket",
    "key": "large-file.zip",
    "bytes_uploaded": 52428800,
    "total_bytes": 104857600,
    "percent": 50,
    "rate_bytes_per_sec": 10485760
  }
}
```

```json
{
  "type": "upload.complete",
  "timestamp": "2024-01-15T10:30:15Z",
  "data": {
    "upload_id": "upload-123",
    "bucket": "my-bucket",
    "key": "large-file.zip",
    "size": 104857600,
    "etag": "e5f6g7h8...",
    "duration_seconds": 10
  }
}
```

##### Node Events

```json
{
  "type": "node.joined",
  "timestamp": "2024-01-15T10:30:00Z",
  "data": {
    "node_id": "node-abc123",
    "peer_id": "12D3KooW...",
    "capacity_gb": 100,
    "location": {
      "datacenter": "us-east-1",
      "rack": "rack-a"
    }
  }
}
```

```json
{
  "type": "node.left",
  "timestamp": "2024-01-15T10:35:00Z",
  "data": {
    "node_id": "node-abc123",
    "reason": "graceful_shutdown"
  }
}
```

##### Repair Events

```json
{
  "type": "repair.started",
  "timestamp": "2024-01-15T10:30:00Z",
  "data": {
    "file_id": "file-123",
    "chunks_affected": 3,
    "reason": "node_failure"
  }
}
```

```json
{
  "type": "repair.complete",
  "timestamp": "2024-01-15T10:31:00Z",
  "data": {
    "file_id": "file-123",
    "chunks_repaired": 3,
    "duration_seconds": 60
  }
}
```

#### Subscription

```json
// Subscribe to specific topics
{
  "type": "subscribe",
  "topics": ["file.*", "upload.progress", "node.joined"]
}

// Unsubscribe
{
  "type": "unsubscribe",
  "topics": ["file.*"]
}
```

#### Heartbeat

```json
// Client sends ping every 30 seconds
{ "type": "ping" }

// Server responds with pong
{ "type": "pong", "timestamp": "2024-01-15T10:30:00Z" }
```

---

### Authentication API

User authentication and API key management.

#### Login

```http
POST /api/v1/auth/login HTTP/1.1
Host: gateway.cyxwiz.com
Content-Type: application/json

{
  "email": "user@example.com",
  "password": "secure-password"
}
```

**Response:**
```json
{
  "user": {
    "id": "user-123",
    "email": "user@example.com",
    "wallet_address": "9aE8...xYz"
  },
  "tokens": {
    "access_token": "eyJhbGciOiJIUzI1NiIs...",
    "refresh_token": "dGhpcyBpcyBhIHJlZnJlc2...",
    "expires_in": 3600
  }
}
```

#### Wallet Connect

```http
POST /api/v1/auth/wallet HTTP/1.1
Host: gateway.cyxwiz.com
Content-Type: application/json

{
  "wallet_address": "9aE8...xYz",
  "signature": "base64-signature",
  "message": "Sign in to CyxCloud at 2024-01-15T10:30:00Z"
}
```

#### API Key Management

```http
POST /api/v1/auth/api-keys HTTP/1.1
Host: gateway.cyxwiz.com
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "CLI Key",
  "permissions": ["read", "write"],
  "expires_at": "2025-01-15T00:00:00Z"
}
```

**Response:**
```json
{
  "id": "key-123",
  "name": "CLI Key",
  "key": "cyxk_live_xxxxxxxxxxxxxxxxxxxx",
  "permissions": ["read", "write"],
  "created_at": "2024-01-15T10:30:00Z",
  "expires_at": "2025-01-15T00:00:00Z"
}
```

---

## Data Structures

### File Metadata

```json
{
  "id": "file-uuid-here",
  "bucket": "my-bucket",
  "key": "data/file.txt",
  "size": 104857600,
  "content_type": "application/octet-stream",
  "etag": "a1b2c3d4e5f6...",
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z",
  "owner_id": "user-123",
  "visibility": "private",
  "encryption": {
    "enabled": true,
    "algorithm": "AES-256-GCM",
    "key_id": "user-key-001"
  },
  "erasure_config": {
    "data_shards": 8,
    "parity_shards": 6,
    "chunk_size": 67108864
  },
  "chunks": [
    {
      "id": "chunk-hash-1",
      "index": 0,
      "size": 67108864,
      "shards": [
        { "index": 0, "node_id": "node-1", "status": "healthy" },
        { "index": 1, "node_id": "node-2", "status": "healthy" },
        // ... 14 shards total
      ]
    }
  ],
  "custom_metadata": {
    "x-cyx-dataset": "imagenet",
    "x-cyx-version": "2024.1"
  }
}
```

### Node Information

```json
{
  "id": "node-abc123",
  "peer_id": "12D3KooWxxxxxx",
  "type": "miner",
  "status": "online",
  "grpc_address": "192.168.1.10:50051",
  "libp2p_address": "/ip4/192.168.1.10/tcp/4001",
  "capacity": {
    "total_bytes": 107374182400,
    "used_bytes": 53687091200,
    "available_bytes": 53687091200,
    "chunk_count": 12847
  },
  "location": {
    "datacenter": "us-east-1",
    "rack": "rack-a",
    "region": "us-east"
  },
  "performance": {
    "read_latency_ms": 1.25,
    "write_latency_ms": 2.10,
    "throughput_mbps": 850
  },
  "economics": {
    "stake_amount": 1000,
    "earnings_total": 5432,
    "uptime_percent": 99.95
  },
  "last_seen": "2024-01-15T10:30:00Z",
  "joined_at": "2023-06-01T00:00:00Z"
}
```

### User Account

```json
{
  "id": "user-123",
  "email": "user@example.com",
  "wallet_address": "9aE8...xYz",
  "created_at": "2024-01-01T00:00:00Z",
  "subscription": {
    "plan": "pro",
    "storage_limit_gb": 100,
    "price_cyxwiz": 40,
    "billing_period": "monthly",
    "current_period_start": "2024-01-01T00:00:00Z",
    "current_period_end": "2024-02-01T00:00:00Z",
    "auto_renew": true
  },
  "usage": {
    "storage_used_bytes": 53687091200,
    "bandwidth_used_bytes": 10737418240,
    "api_requests": 45678
  },
  "quotas": {
    "storage_bytes": 107374182400,
    "bandwidth_bytes_per_month": 1099511627776,
    "api_requests_per_month": 1000000
  }
}
```

---

## Protocol Definitions

### Protobuf Files

The project uses Protocol Buffers for gRPC service definitions.

```
cyxcloud-protocol/proto/
├── chunk.proto        # Chunk storage operations
├── metadata.proto     # File and bucket metadata
├── node.proto         # Node registration and health
└── gateway.proto      # High-level gateway operations
```

#### Node Service Protocol

```protobuf
// node.proto
syntax = "proto3";
package cyxcloud.node;

service NodeService {
  // Register node with gateway
  rpc Register(RegisterRequest) returns (RegisterResponse);

  // Send periodic heartbeat
  rpc Heartbeat(HeartbeatRequest) returns (HeartbeatResponse);

  // Report metrics
  rpc ReportMetrics(MetricsRequest) returns (MetricsResponse);

  // Get assigned work
  rpc GetWork(GetWorkRequest) returns (stream WorkItem);
}

message RegisterRequest {
  string node_id = 1;
  string peer_id = 2;
  string grpc_address = 3;
  string libp2p_address = 4;
  NodeCapacity capacity = 5;
  NodeLocation location = 6;
  NodeType type = 7;
}

enum NodeType {
  NODE_TYPE_UNSPECIFIED = 0;
  NODE_TYPE_MINER = 1;
  NODE_TYPE_VOLUNTEER = 2;
  NODE_TYPE_ENTERPRISE = 3;
}

message NodeCapacity {
  uint64 total_bytes = 1;
  uint64 available_bytes = 2;
}

message NodeLocation {
  string datacenter = 1;
  string rack = 2;
  string region = 3;
  double latitude = 4;
  double longitude = 5;
}

message RegisterResponse {
  bool success = 1;
  string error_message = 2;
  string auth_token = 3;  // Token for future requests
}

message HeartbeatRequest {
  string node_id = 1;
  string auth_token = 2;
  NodeStatus status = 3;
}

message HeartbeatResponse {
  bool acknowledged = 1;
  repeated string commands = 2;  // Commands to execute (e.g., "drain", "shutdown")
}

message NodeStatus {
  uint64 used_bytes = 1;
  uint64 chunk_count = 2;
  double cpu_percent = 3;
  double memory_percent = 4;
  double disk_read_mbps = 5;
  double disk_write_mbps = 6;
  double network_in_mbps = 7;
  double network_out_mbps = 8;
}
```

---

## Node Fault Tolerance

CyxCloud implements automatic node lifecycle management to ensure data availability even when storage nodes go offline.

### Node State Machine

```
                    heartbeat received
                    ┌──────────────────────┐
                    │                      │
                    ▼                      │
┌────────┐    ┌──────────┐   5 min    ┌─────────┐   4 hours   ┌──────────┐   7 days   ┌─────────┐
│Register│───▶│  ONLINE  │───────────▶│ OFFLINE │────────────▶│ DRAINING │───────────▶│ REMOVED │
└────────┘    └──────────┘ no beat    └────┬────┘             └────┬─────┘            └─────────┘
                    ▲                      │ heartbeat              │ heartbeat
                    │ 5 min quarantine     ▼                        ▼
              ┌───────────────────────────────────────────────────────┐
              │                     RECOVERING                         │
              └───────────────────────────────────────────────────────┘
```

### Node Status States

| Status | Description | Can Read | Can Write |
|--------|-------------|----------|-----------|
| **ONLINE** | Healthy, sending heartbeats | ✅ | ✅ |
| **OFFLINE** | No heartbeat for 5+ minutes | ❌ | ❌ |
| **RECOVERING** | Reconnected, in 5-min quarantine | ✅ | ❌ |
| **DRAINING** | Offline 4+ hours, chunks evacuating | ❌ | ❌ |
| **MAINTENANCE** | Manually set, graceful shutdown | ✅ | ❌ |

### Automatic Lifecycle Transitions

1. **Online → Offline**: After 5 minutes without heartbeat
   - Node marked offline
   - Chunks still counted but not served

2. **Offline → Draining**: After 4 hours offline
   - Chunk evacuation begins
   - Repair jobs created for each chunk on the node
   - Chunks copied to healthy nodes

3. **Draining → Removed**: After 7 days offline
   - Node deleted from database
   - Chunk locations cascade deleted
   - Node must re-register to rejoin

4. **Offline/Draining → Recovering**: When heartbeat received
   - Node enters 5-minute quarantine
   - Can serve existing chunks (read-healthy)
   - Cannot receive new chunks (not write-healthy)

5. **Recovering → Online**: After 5-minute quarantine completes
   - Node fully online
   - Can read and write chunks

### Configuration

Fault tolerance thresholds are configurable via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `NODE_OFFLINE_THRESHOLD_SECS` | 300 (5 min) | Time without heartbeat before marking offline |
| `NODE_DRAIN_THRESHOLD_SECS` | 14400 (4 hr) | Time offline before starting chunk evacuation |
| `NODE_REMOVE_THRESHOLD_SECS` | 604800 (7 days) | Time offline before auto-removal |
| `NODE_RECOVERY_QUARANTINE_SECS` | 300 (5 min) | Quarantine period for reconnecting nodes |
| `NODE_MONITOR_INTERVAL_SECS` | 30 | How often the monitor checks node status |

### Chunk Evacuation Process

When a node starts draining:

1. **Detection**: Node monitor detects node has been offline > 4 hours
2. **Status Update**: Node marked as `draining`
3. **Chunk Discovery**: All chunks on the draining node are identified
4. **Repair Job Creation**: For each chunk:
   - Source: draining node (if still reachable) or other replica
   - Target: healthy online node (round-robin selection)
   - Priority: 100 (high priority for evacuation)
5. **Rebalancer Execution**: Repair jobs executed by rebalancer service
6. **Replica Update**: Chunk locations updated as copies complete

### Node Monitor Architecture

```
+─────────────────────────────────────────────────────────────+
│                     Gateway Process                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Node Monitor                        │   │
│  │  +──────────────────────────────────────────────+   │   │
│  │  │              Check Cycle (every 30s)          │   │   │
│  │  │                                               │   │   │
│  │  │  1. Find stale online nodes → mark OFFLINE    │   │   │
│  │  │  2. Find nodes offline > 4hr → mark DRAINING  │   │   │
│  │  │  3. Find nodes offline > 7d → DELETE          │   │   │
│  │  │  4. Find recovered nodes → mark ONLINE        │   │   │
│  │  │                                               │   │   │
│  │  +──────────────────────────────────────────────+   │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                  Metadata Service                    │   │
│  │                   (PostgreSQL)                       │   │
│  └─────────────────────────────────────────────────────┘   │
+─────────────────────────────────────────────────────────────+
```

### Database Schema Extensions

The fault tolerance system adds two columns to the `nodes` table:

```sql
-- Track when node first went offline
first_offline_at TIMESTAMP WITH TIME ZONE;

-- Track when status last changed
status_changed_at TIMESTAMP WITH TIME ZONE DEFAULT NOW();
```

---

## Storage Architecture

### Erasure Coding Configuration

```
+------------------------------------------------------------------------------+
|                         Erasure Coding Profiles                               |
+------------------------------------------------------------------------------+
|                                                                                |
|  Profile          Data  Parity  Tolerance  Overhead  Use Case                 |
|  ───────────────────────────────────────────────────────────────────────────  |
|  basic            8     4       4 nodes    50%       Cost-effective            |
|  standard         8     6       6 nodes    75%       General purpose (default) |
|  high-durability  8     8       8 nodes    100%      Critical data             |
|  extreme          6     10      10 nodes   167%      Financial/medical         |
|  efficient        10    4       4 nodes    40%       Large archives            |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Chunk Format

```
+------------------------------------------------------------------------------+
|                            Chunk Binary Format                                 |
+------------------------------------------------------------------------------+
|                                                                                |
|  Offset   Size    Field           Description                                 |
|  ──────────────────────────────────────────────────────────────────────────   |
|  0        4       Magic           0x43595843 ("CYXC")                         |
|  4        4       Version         Format version (1)                          |
|  8        4       Flags           Bit flags (encrypted, compressed)           |
|  12       8       Original Size   Size before processing                      |
|  20       8       Stored Size     Size in storage                             |
|  28       32      Content Hash    Blake3 hash of content                      |
|  60       12      Reserved        Future use                                  |
|  72       N       Data            Chunk data (possibly encrypted)             |
|                                                                                |
|  Flags:                                                                       |
|    Bit 0: Encrypted (AES-256-GCM)                                             |
|    Bit 1: Compressed (LZ4)                                                    |
|    Bit 2: Verified (integrity checked)                                        |
|    Bits 3-31: Reserved                                                        |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Storage Backend Interface

```rust
/// Storage backend trait for chunk persistence
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Store a chunk
    async fn put(&self, chunk_id: &ChunkId, data: &[u8]) -> Result<()>;

    /// Retrieve a chunk
    async fn get(&self, chunk_id: &ChunkId) -> Result<Option<Vec<u8>>>;

    /// Delete a chunk
    async fn delete(&self, chunk_id: &ChunkId) -> Result<bool>;

    /// Check if chunk exists
    async fn exists(&self, chunk_id: &ChunkId) -> Result<bool>;

    /// Verify chunk integrity
    async fn verify(&self, chunk_id: &ChunkId, expected_hash: &[u8]) -> Result<bool>;

    /// List all chunk IDs
    async fn list(&self) -> Result<Vec<ChunkId>>;

    /// Get storage statistics
    async fn stats(&self) -> Result<StorageStats>;
}

pub struct StorageStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub chunk_count: u64,
    pub read_latency_us: u64,
    pub write_latency_us: u64,
}
```

---

## Network Architecture

### Hybrid Networking Model

```
+------------------------------------------------------------------------------+
|                         Network Layer Architecture                             |
+------------------------------------------------------------------------------+
|                                                                                |
|  +----------------------------------+  +----------------------------------+   |
|  |           gRPC Layer             |  |          libp2p Layer            |   |
|  +----------------------------------+  +----------------------------------+   |
|  |                                  |  |                                  |   |
|  |  Purpose: Data Transfer          |  |  Purpose: Peer Discovery         |   |
|  |                                  |  |                                  |   |
|  |  - ChunkService (store/get)      |  |  - Kademlia DHT                  |   |
|  |  - Streaming uploads             |  |  - Identify protocol             |   |
|  |  - Streaming downloads           |  |  - Ping protocol                 |   |
|  |  - Health checks                 |  |  - mDNS (LAN discovery)          |   |
|  |                                  |  |                                  |   |
|  |  Transport: TCP + TLS 1.3        |  |  Transport: QUIC                 |   |
|  |  Port: 50051 (default)           |  |  Port: 4001 (default)            |   |
|  |                                  |  |                                  |   |
|  |  Connection: Pooled,             |  |  Connection: Persistent          |   |
|  |              Multiplexed         |  |              Swarm                |   |
|  |                                  |  |                                  |   |
|  +----------------------------------+  +----------------------------------+   |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Node Discovery Flow

```
+------------------------------------------------------------------------------+
|                           Node Discovery Flow                                  |
+------------------------------------------------------------------------------+
|                                                                                |
|  1. Node starts up                                                            |
|     +------------------+                                                      |
|     |  Storage Node    |                                                      |
|     +--------+---------+                                                      |
|              |                                                                 |
|              v                                                                 |
|  2. Connect to bootstrap peers                                                |
|     +------------------+     +------------------+     +------------------+    |
|     | Bootstrap Peer 1 |     | Bootstrap Peer 2 |     | Bootstrap Peer 3 |    |
|     +--------+---------+     +--------+---------+     +--------+---------+    |
|              |                        |                        |              |
|              +------------------------+------------------------+              |
|                                       |                                        |
|                                       v                                        |
|  3. Join Kademlia DHT                                                         |
|     +----------------------------------------------------------------------+  |
|     |                         Kademlia DHT                                  |  |
|     |  +------------+  +------------+  +------------+  +------------+      |  |
|     |  | Node Table |  | Node Table |  | Node Table |  | Node Table |      |  |
|     |  +------------+  +------------+  +------------+  +------------+      |  |
|     +----------------------------------------------------------------------+  |
|                                       |                                        |
|                                       v                                        |
|  4. Announce presence (PUT to DHT)                                            |
|     Key: /cyxcloud/nodes/{node_id}                                            |
|     Value: NodeAnnouncement { grpc_addr, capacity, location }                 |
|                                       |                                        |
|                                       v                                        |
|  5. Register with Gateway                                                     |
|     +------------------+                                                      |
|     |     Gateway      |  <-- gRPC: Register(node_info)                       |
|     +------------------+                                                      |
|                                                                                |
+------------------------------------------------------------------------------+
```

---

## Security Architecture

### Authentication Flow

```
+------------------------------------------------------------------------------+
|                         Authentication Architecture                            |
+------------------------------------------------------------------------------+
|                                                                                |
|  Client Authentication (Users)                                                |
|  ────────────────────────────                                                 |
|                                                                                |
|  +--------+     +----------+     +----------+     +----------+               |
|  | Client |---->| Auth     |---->| JWT      |---->| Gateway  |               |
|  |        |     | Service  |     | Token    |     |          |               |
|  +--------+     +----------+     +----------+     +----------+               |
|       |              |                                  |                     |
|       |   Login      |   Generate                       |   Validate          |
|       |   Request    |   JWT                            |   Token             |
|                                                                                |
|  JWT Claims:                                                                  |
|  {                                                                            |
|    "sub": "user-123",                                                         |
|    "email": "user@example.com",                                               |
|    "wallet": "9aE8...xYz",                                                    |
|    "permissions": ["read", "write"],                                          |
|    "exp": 1705320000,                                                         |
|    "iss": "cyxcloud"                                                          |
|  }                                                                            |
|                                                                                |
|  Node Authentication (Storage Nodes)                                          |
|  ────────────────────────────────────                                         |
|                                                                                |
|  +--------+     +----------+     +----------+     +----------+               |
|  | Node   |---->| Register |---->| Auth     |---->| Gateway  |               |
|  |        |     | + Stake  |     | Token    |     |          |               |
|  +--------+     +----------+     +----------+     +----------+               |
|       |              |                                  |                     |
|       |   Ed25519    |   Verify Stake                   |   Mutual TLS        |
|       |   Signature  |   Issue Token                    |   + Token           |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Encryption Pipeline

```
+------------------------------------------------------------------------------+
|                         Encryption Architecture                                |
+------------------------------------------------------------------------------+
|                                                                                |
|  Upload Path (Client-Side Encryption)                                         |
|  ────────────────────────────────────                                         |
|                                                                                |
|  +----------+     +----------+     +----------+     +----------+             |
|  | Plaintext|---->| Compress |---->| Encrypt  |---->| Erasure  |             |
|  |   Data   |     | (LZ4)    |     | (AES-GCM)|     | Encode   |             |
|  +----------+     +----------+     +----------+     +----------+             |
|                                          |                                    |
|                                          v                                    |
|                                   +------------+                              |
|                                   | Encryption |                              |
|                                   | Metadata   |                              |
|                                   +------------+                              |
|                                   | algorithm: AES-256-GCM                    |
|                                   | key_id: user-key-001                      |
|                                   | nonce: (random 12 bytes)                  |
|                                   | tag: (16 bytes auth tag)                  |
|                                   +------------+                              |
|                                                                                |
|  Key Derivation:                                                              |
|  ───────────────                                                              |
|                                                                                |
|  Master Key (User Password or Hardware Key)                                   |
|       |                                                                        |
|       v                                                                        |
|  HKDF-SHA256(master_key, salt="cyxcloud-v1")                                  |
|       |                                                                        |
|       v                                                                        |
|  Derived Key (256-bit AES key)                                                |
|       |                                                                        |
|       +---> Per-Chunk Key = HKDF(derived_key, chunk_id)                       |
|                                                                                |
|  Key Management Options:                                                      |
|  ───────────────────────                                                      |
|  1. Password-based (PBKDF2)                                                   |
|  2. Hardware wallet (Ed25519 to X25519)                                       |
|  3. KMS-managed (AWS KMS, GCP KMS)                                            |
|  4. Self-managed (bring your own key)                                         |
|                                                                                |
+------------------------------------------------------------------------------+
```

---

## Token Economics & Payments

### CYXWIZ Token Flow

```
+------------------------------------------------------------------------------+
|                           Token Flow Diagram                                   |
+------------------------------------------------------------------------------+
|                                                                                |
|                        +---------------------+                                 |
|                        |      User Pays      |                                 |
|                        |   (Storage + BW)    |                                 |
|                        +---------+-----------+                                 |
|                                  |                                             |
|                                  v                                             |
|                        +---------------------+                                 |
|                        |   Payment Escrow    |                                 |
|                        |   (Solana Program)  |                                 |
|                        +---------+-----------+                                 |
|                                  |                                             |
|            +--------------------+|+--------------------+                       |
|            |                     |                     |                       |
|            v                     v                     v                       |
|   +----------------+    +----------------+    +----------------+              |
|   |  Storage Nodes |    |    Protocol    |    |   Community    |              |
|   |     (85%)      |    |   Treasury     |    |     Fund       |              |
|   |                |    |     (10%)      |    |     (5%)       |              |
|   +----------------+    +----------------+    +----------------+              |
|            |                     |                     |                       |
|            v                     v                     v                       |
|   +----------------+    +----------------+    +----------------+              |
|   | Miner Rewards  |    |  Development   |    | Public Datasets|              |
|   | (per chunk)    |    |  + Operations  |    | + Grants       |              |
|   +----------------+    +----------------+    +----------------+              |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Storage Pricing Tiers

```
+------------------------------------------------------------------------------+
|                         Storage Pricing Tiers                                  |
+------------------------------------------------------------------------------+
|                                                                                |
|  Plan         | Capacity | Monthly (CYXWIZ) | Yearly (20% off) | Features     |
|  ─────────────|──────────|──────────────────|──────────────────|──────────────|
|  Free         | 1 GB     | 0                | 0                | Public only  |
|  Basic        | 10 GB    | 5                | 48               | Private      |
|  Pro          | 100 GB   | 40               | 384              | + Sharing    |
|  Team         | 500 GB   | 150              | 1,440            | + Teams      |
|  Enterprise   | Custom   | Custom           | Custom           | + SLA        |
|                                                                                |
|  Additional Costs:                                                            |
|  ─────────────────                                                            |
|  • Bandwidth (egress): 0.001 CYXWIZ / GB                                      |
|  • API requests: Free (first 1M/mo), then 0.0001 CYXWIZ / 1K                  |
|  • Premium redundancy (8+8): +50% storage cost                                |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Payment Processing

```rust
// Solana program for storage payments
#[program]
pub mod cyxcloud_payments {
    use anchor_lang::prelude::*;

    #[derive(Accounts)]
    pub struct CreateSubscription<'info> {
        #[account(mut)]
        pub user: Signer<'info>,
        #[account(init, payer = user, space = Subscription::SIZE)]
        pub subscription: Account<'info, Subscription>,
        #[account(mut)]
        pub user_token_account: Account<'info, TokenAccount>,
        #[account(mut)]
        pub escrow_account: Account<'info, TokenAccount>,
        pub token_program: Program<'info, Token>,
        pub system_program: Program<'info, System>,
    }

    pub fn create_subscription(
        ctx: Context<CreateSubscription>,
        plan: SubscriptionPlan,
        duration_months: u8,
    ) -> Result<()> {
        let subscription = &mut ctx.accounts.subscription;
        subscription.user = ctx.accounts.user.key();
        subscription.plan = plan;
        subscription.start_time = Clock::get()?.unix_timestamp;
        subscription.end_time = subscription.start_time + (duration_months as i64 * 30 * 24 * 3600);

        // Transfer tokens to escrow
        let amount = plan.price_per_month() * duration_months as u64;
        token::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.user_token_account.to_account_info(),
                    to: ctx.accounts.escrow_account.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            amount,
        )?;

        Ok(())
    }
}
```

---

## CyxHub Architecture

### Public Dataset Library

```
+------------------------------------------------------------------------------+
|                           CyxHub Architecture                                  |
+------------------------------------------------------------------------------+
|                                                                                |
|  +------------------------------------------------------------------------+   |
|  |                          CyxHub Frontend                                |   |
|  |  +------------------+  +------------------+  +------------------+      |   |
|  |  |     Browse       |  |     Search       |  |     Upload       |      |   |
|  |  |    Datasets      |  |    Datasets      |  |    Dataset       |      |   |
|  |  +------------------+  +------------------+  +------------------+      |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|                                    v                                           |
|  +------------------------------------------------------------------------+   |
|  |                          CyxHub API                                     |   |
|  |  +------------------+  +------------------+  +------------------+      |   |
|  |  |   Dataset        |  |   Search         |  |   Moderation     |      |   |
|  |  |   Registry       |  |   Service        |  |   Service        |      |   |
|  |  +------------------+  +------------------+  +------------------+      |   |
|  +------------------------------------------------------------------------+   |
|                                    |                                           |
|         +--------------------------|---------------------------+              |
|         |                          |                           |              |
|         v                          v                           v              |
|  +----------------+       +----------------+       +----------------+         |
|  |   PostgreSQL   |       |  Meilisearch   |       |    CyxCloud    |         |
|  |   (metadata)   |       |   (search)     |       |   (storage)    |         |
|  +----------------+       +----------------+       +----------------+         |
|                                                                                |
+------------------------------------------------------------------------------+
```

### Dataset Types

```
+------------------------------------------------------------------------------+
|                            Dataset Categories                                  |
+------------------------------------------------------------------------------+
|                                                                                |
|  Free Public Datasets (Community-Funded)                                      |
|  ───────────────────────────────────────                                      |
|  • MNIST, CIFAR-10, Fashion-MNIST                                             |
|  • Wikipedia dumps                                                            |
|  • Common Crawl subsets                                                       |
|  • Academic datasets (CC-BY, MIT)                                             |
|                                                                                |
|  Premium Datasets (Pay-per-Download)                                          |
|  ────────────────────────────────────                                         |
|  • High-quality labeled datasets                                              |
|  • Medical imaging                                                            |
|  • Satellite imagery                                                          |
|  • Proprietary research data                                                  |
|                                                                                |
|  Volunteer-Hosted Datasets                                                    |
|  ─────────────────────────────                                                |
|  • Hosted on volunteer nodes (no cost)                                        |
|  • Best-effort availability                                                   |
|  • Community-curated                                                          |
|  • May have lower redundancy                                                  |
|                                                                                |
+------------------------------------------------------------------------------+
```

---

## Error Handling

### Error Codes

| Category | Code | HTTP | Description |
|----------|------|------|-------------|
| **Authentication** | | | |
| | `AUTH_INVALID_TOKEN` | 401 | JWT token invalid or expired |
| | `AUTH_INSUFFICIENT_PERMISSIONS` | 403 | Missing required permissions |
| | `AUTH_WALLET_MISMATCH` | 403 | Wallet signature invalid |
| **Storage** | | | |
| | `STORAGE_BUCKET_NOT_FOUND` | 404 | Bucket does not exist |
| | `STORAGE_OBJECT_NOT_FOUND` | 404 | Object does not exist |
| | `STORAGE_QUOTA_EXCEEDED` | 507 | Storage quota exhausted |
| | `STORAGE_BUCKET_NOT_EMPTY` | 409 | Cannot delete non-empty bucket |
| **Network** | | | |
| | `NETWORK_NO_NODES_AVAILABLE` | 503 | No storage nodes online |
| | `NETWORK_QUORUM_FAILED` | 503 | Could not reach write quorum |
| | `NETWORK_NODE_TIMEOUT` | 504 | Node did not respond |
| **Payment** | | | |
| | `PAYMENT_INSUFFICIENT_BALANCE` | 402 | Not enough CYXWIZ tokens |
| | `PAYMENT_SUBSCRIPTION_EXPIRED` | 402 | Subscription has lapsed |
| | `PAYMENT_TRANSACTION_FAILED` | 500 | Blockchain transaction failed |

### Error Response Format

```json
{
  "error": {
    "code": "STORAGE_QUOTA_EXCEEDED",
    "message": "Storage quota of 10 GB exceeded. Current usage: 10.5 GB",
    "details": {
      "quota_bytes": 10737418240,
      "used_bytes": 11274289152,
      "upgrade_url": "https://cyxwiz.com/upgrade"
    },
    "request_id": "req-abc123",
    "timestamp": "2024-01-15T10:30:00Z"
  }
}
```

---

## Configuration Reference

### Gateway Configuration

```toml
# gateway.toml

[server]
host = "0.0.0.0"
http_port = 8080
grpc_port = 50050
websocket_enabled = true

[database]
postgres_url = "postgres://cyxcloud:password@localhost/cyxcloud"
redis_url = "redis://localhost:6379"

[storage]
default_erasure_data_shards = 8
default_erasure_parity_shards = 6
default_chunk_size_bytes = 67108864  # 64 MB
max_upload_size_bytes = 5368709120   # 5 GB

[auth]
jwt_secret = "your-secret-key"
jwt_expiry_seconds = 3600
api_key_prefix = "cyxk_"

[blockchain]
solana_rpc_url = "https://api.mainnet-beta.solana.com"
cyxwiz_token_mint = "CYXWiz..."
escrow_program_id = "CYXESC..."

[logging]
level = "info"
format = "json"
```

### Node Configuration

```toml
# node.toml

[node]
id = "node-abc123"
type = "miner"  # miner | volunteer | enterprise

[storage]
path = "/data/chunks"
capacity_gb = 100
backend = "rocksdb"  # rocksdb | sled | memory

[network]
grpc_host = "0.0.0.0"
grpc_port = 50051
libp2p_port = 4001
bootstrap_peers = [
    "/ip4/192.168.1.10/tcp/4001/p2p/12D3KooW...",
    "/ip4/192.168.1.11/tcp/4001/p2p/12D3KooW...",
]

[gateway]
url = "https://gateway.cyxwiz.com"
auth_token = "node-auth-token"

[economics]
stake_wallet = "9aE8...xYz"
payout_address = "8bF9...aBc"

[logging]
level = "info"
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Logging level |
| `GATEWAY_HOST` | `0.0.0.0` | Gateway bind address |
| `GATEWAY_PORT` | `8080` | Gateway HTTP port |
| `DATABASE_URL` | - | PostgreSQL connection string |
| `REDIS_URL` | - | Redis connection string |
| `JWT_SECRET` | - | JWT signing secret |
| `SOLANA_RPC_URL` | - | Solana RPC endpoint |
| `NODE_ID` | `node-1` | Unique node identifier |
| `STORAGE_PATH` | `/data/chunks` | Chunk storage directory |

---

## Appendix: API Quick Reference

### S3 API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| `PUT` | `/s3/{bucket}` | Create bucket |
| `DELETE` | `/s3/{bucket}` | Delete bucket |
| `GET` | `/s3/{bucket}?list-type=2` | List objects |
| `PUT` | `/s3/{bucket}/{key}` | Upload object |
| `GET` | `/s3/{bucket}/{key}` | Download object |
| `HEAD` | `/s3/{bucket}/{key}` | Get object metadata |
| `DELETE` | `/s3/{bucket}/{key}` | Delete object |

### gRPC Services

| Service | Method | Description |
|---------|--------|-------------|
| `ChunkService` | `StoreChunk` | Store single chunk |
| `ChunkService` | `GetChunk` | Retrieve single chunk |
| `ChunkService` | `DeleteChunk` | Delete chunk |
| `ChunkService` | `StreamChunks` | Upload multiple chunks |
| `ChunkService` | `FetchChunks` | Download multiple chunks |
| `ChunkService` | `VerifyChunk` | Verify integrity |
| `ChunkService` | `HealthCheck` | Check node health |
| `NodeService` | `Register` | Register node |
| `NodeService` | `Heartbeat` | Send heartbeat |
| `NodeService` | `ReportMetrics` | Report metrics |

### WebSocket Events

| Event | Direction | Description |
|-------|-----------|-------------|
| `auth` | Client → Server | Authenticate connection |
| `subscribe` | Client → Server | Subscribe to topics |
| `unsubscribe` | Client → Server | Unsubscribe from topics |
| `ping` | Client → Server | Keep-alive ping |
| `pong` | Server → Client | Keep-alive response |
| `file.created` | Server → Client | File created |
| `file.deleted` | Server → Client | File deleted |
| `upload.progress` | Server → Client | Upload progress |
| `upload.complete` | Server → Client | Upload finished |
| `node.joined` | Server → Client | Node joined network |
| `node.left` | Server → Client | Node left network |
| `repair.started` | Server → Client | Chunk repair started |
| `repair.complete` | Server → Client | Chunk repair finished |

---

*This document is the authoritative source for CyxCloud technical architecture. Last updated: 2024-01-15*
