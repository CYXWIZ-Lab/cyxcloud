# CyxCloud DataStream Architecture

## Overview

DataStream enables zero-copy data streaming from CyxCloud directly to GPU memory during ML training, with cryptographic verification at every step.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              CyxWiz Engine                               │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────────────┐  │
│  │ Cloud      │  │ Dataset    │  │ Training   │  │ Verification     │  │
│  │ Browser    │  │ Manager    │  │ Controller │  │ Status           │  │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └────────┬─────────┘  │
└────────┼───────────────┼───────────────┼──────────────────┼────────────┘
         │               │               │                  │
         ▼               ▼               ▼                  ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                           CyxCloud Gateway                               │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────────────┐  │
│  │ S3 API     │  │ DataStream │  │ Auth/Token │  │ Verification     │  │
│  │            │  │ API        │  │ Service    │  │ Service          │  │
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └────────┬─────────┘  │
└────────┼───────────────┼───────────────┼──────────────────┼────────────┘
         │               │               │                  │
         ▼               ▼               ▼                  ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                           Storage Layer                                  │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐                        │
│  │ Node 1     │  │ Node 2     │  │ Node 3     │  (Erasure-coded shards)│
│  └────────────┘  └────────────┘  └────────────┘                        │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Core Concepts

### 1. Zero-Copy Streaming

Data flows directly from storage nodes to GPU memory without intermediate disk writes:

```
Storage Nodes → Gateway (erasure decode) → gRPC stream → Server Node → GPU
     │                    │                     │              │
   Shards            In-memory              Batches      Training
                     assembly                            tensor
```

### 2. Content-Addressed Verification

Every piece of data is verified by its cryptographic hash:

| Level | Hash | Purpose |
|-------|------|---------|
| File | Blake3(entire file) | Dataset identity |
| Chunk | Blake3(chunk data) | Chunk integrity |
| Shard | Blake3(shard data) | Storage verification |
| Batch | Blake3(batch data) | Training verification |

### 3. Trust Levels

```
┌─────────────────────────────────────────────────────────────┐
│                      Trust Hierarchy                         │
├─────────────────────────────────────────────────────────────┤
│ Level 0: SELF        │ User's own uploads                   │
│ Level 1: SIGNED      │ Data signed by known identity        │
│ Level 2: VERIFIED    │ Hash matches public registry         │
│ Level 3: ATTESTED    │ Third-party verification             │
│ Level 4: UNTRUSTED   │ Unknown source (requires audit)      │
└─────────────────────────────────────────────────────────────┘
```

---

## Architecture Components

### 1. Dataset Registry

Manages dataset metadata, versions, and trust information.

```rust
// cyxcloud-metadata/src/models.rs

/// A dataset is a versioned collection of files for ML training
pub struct Dataset {
    pub id: Uuid,
    pub name: String,
    pub owner_id: Uuid,
    pub description: Option<String>,

    // Content addressing
    pub content_hash: Vec<u8>,       // Blake3 hash of manifest
    pub total_size_bytes: i64,
    pub file_count: i32,

    // Schema (for structured data)
    pub schema: Option<serde_json::Value>,  // Column types, shapes

    // Trust
    pub trust_level: TrustLevel,
    pub signature: Option<Vec<u8>>,   // Owner's Ed25519 signature
    pub verified_at: Option<DateTime<Utc>>,

    // Versioning
    pub version: i32,
    pub parent_version_id: Option<Uuid>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum TrustLevel {
    Self_ = 0,      // User's own data
    Signed = 1,     // Signed by known identity
    Verified = 2,   // Matches public registry
    Attested = 3,   // Third-party verified
    Untrusted = 4,  // Unknown source
}

/// Individual file within a dataset
pub struct DatasetFile {
    pub id: Uuid,
    pub dataset_id: Uuid,
    pub file_id: Uuid,           // References files table
    pub path_in_dataset: String, // e.g., "train/images/001.jpg"
    pub content_hash: Vec<u8>,
    pub size_bytes: i64,
    pub file_index: i32,         // Order in dataset
}

/// Public dataset registry (ImageNet, CIFAR, etc.)
pub struct PublicDataset {
    pub id: Uuid,
    pub name: String,            // "imagenet-1k"
    pub version: String,         // "2012"
    pub official_url: String,    // Original download source
    pub official_hash: Vec<u8>,  // Published hash
    pub paper_url: Option<String>,
    pub license: String,

    // CyxCloud cached copy
    pub cached_dataset_id: Option<Uuid>,
    pub cached_at: Option<DateTime<Utc>>,

    // Verification
    pub verified_by: Vec<String>, // ["torchvision", "huggingface"]
    pub verified_at: DateTime<Utc>,
}
```

### 2. DataStream Service

Handles streaming data to training nodes with verification.

```rust
// cyxcloud-gateway/src/datastream.rs

use cyxcloud_core::Blake3Hasher;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status, Streaming};

pub struct DataStreamService {
    state: Arc<AppState>,
    hasher: Blake3Hasher,
}

impl DataStreamService {
    /// Stream dataset batches for training
    pub async fn stream_batches(
        &self,
        request: Request<StreamBatchesRequest>,
    ) -> Result<Response<Streaming<BatchResponse>>, Status> {
        let req = request.into_inner();

        // Verify access token
        let token = self.verify_access_token(&req.access_token).await?;

        // Get dataset metadata
        let dataset = self.state.metadata()
            .get_dataset(req.dataset_id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;

        // Verify trust level is acceptable
        if dataset.trust_level as i32 > req.max_trust_level {
            return Err(Status::permission_denied(
                format!("Dataset trust level {} exceeds maximum {}",
                    dataset.trust_level as i32, req.max_trust_level)
            ));
        }

        // Create streaming channel
        let (tx, rx) = mpsc::channel(req.prefetch_batches as usize);

        // Spawn batch producer
        let state = self.state.clone();
        let hasher = self.hasher.clone();
        tokio::spawn(async move {
            Self::produce_batches(state, hasher, dataset, req, tx).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn produce_batches(
        state: Arc<AppState>,
        hasher: Blake3Hasher,
        dataset: Dataset,
        req: StreamBatchesRequest,
        tx: mpsc::Sender<Result<BatchResponse, Status>>,
    ) {
        let batch_size = req.batch_size as usize;
        let mut file_index = req.start_index as usize;
        let mut batch_index = 0u64;

        let files = state.metadata()
            .get_dataset_files(dataset.id)
            .await
            .unwrap();

        while file_index < files.len() {
            // Collect batch
            let batch_files: Vec<_> = files[file_index..]
                .iter()
                .take(batch_size)
                .collect();

            let mut batch_data = Vec::new();
            let mut batch_hashes = Vec::new();

            for file in &batch_files {
                // Fetch file data (erasure decode internally)
                let data = state.get_object_verified(
                    &dataset.owner_id.to_string(),
                    &file.path_in_dataset,
                    &file.content_hash,
                ).await;

                match data {
                    Ok(bytes) => {
                        batch_hashes.push(file.content_hash.clone());
                        batch_data.push(bytes);
                    }
                    Err(e) => {
                        let _ = tx.send(Err(Status::internal(
                            format!("Verification failed for {}: {}",
                                file.path_in_dataset, e)
                        ))).await;
                        return;
                    }
                }
            }

            // Compute batch hash
            let mut batch_hasher = hasher.clone();
            for data in &batch_data {
                batch_hasher.update(data);
            }
            let batch_hash = batch_hasher.finalize();

            // Send batch
            let response = BatchResponse {
                batch_index,
                items: batch_data.into_iter().map(|b| b.to_vec()).collect(),
                item_hashes: batch_hashes,
                batch_hash: batch_hash.to_vec(),
                total_batches: (files.len() / batch_size) as u64,
                is_last: file_index + batch_size >= files.len(),
            };

            if tx.send(Ok(response)).await.is_err() {
                break; // Client disconnected
            }

            file_index += batch_size;
            batch_index += 1;
        }
    }

    /// Verify data access token
    async fn verify_access_token(&self, token: &str) -> Result<DataAccessToken, Status> {
        self.state.metadata()
            .verify_data_access_token(token)
            .await
            .map_err(|e| Status::unauthenticated(e.to_string()))
    }
}
```

### 3. Verification Service

Handles hash verification and trust attestation.

```rust
// cyxcloud-gateway/src/verification.rs

pub struct VerificationService {
    state: Arc<AppState>,
    public_registry: Arc<PublicDatasetRegistry>,
}

impl VerificationService {
    /// Verify a dataset against known hashes
    pub async fn verify_dataset(
        &self,
        dataset_id: Uuid,
    ) -> Result<VerificationResult, Error> {
        let dataset = self.state.metadata()
            .get_dataset(dataset_id)
            .await?;

        let files = self.state.metadata()
            .get_dataset_files(dataset_id)
            .await?;

        let mut results = Vec::new();
        let mut all_valid = true;

        for file in files {
            // Get stored file
            let stored_file = self.state.metadata()
                .get_file(file.file_id)
                .await?;

            // Verify content hash matches
            let valid = stored_file.content_hash == file.content_hash;
            if !valid {
                all_valid = false;
            }

            results.push(FileVerification {
                path: file.path_in_dataset,
                expected_hash: hex::encode(&file.content_hash),
                actual_hash: hex::encode(&stored_file.content_hash),
                valid,
            });
        }

        // Compute manifest hash
        let manifest_hash = self.compute_manifest_hash(&files);
        let manifest_valid = manifest_hash == dataset.content_hash;

        Ok(VerificationResult {
            dataset_id,
            manifest_valid,
            files: results,
            all_valid: all_valid && manifest_valid,
            verified_at: Utc::now(),
        })
    }

    /// Check if dataset matches a public registry entry
    pub async fn verify_against_public_registry(
        &self,
        dataset_id: Uuid,
    ) -> Result<Option<PublicDatasetMatch>, Error> {
        let dataset = self.state.metadata()
            .get_dataset(dataset_id)
            .await?;

        // Search public registry by hash
        let matches = self.public_registry
            .find_by_hash(&dataset.content_hash)
            .await?;

        if let Some(public_dataset) = matches.first() {
            // Update trust level
            self.state.metadata()
                .update_dataset_trust(
                    dataset_id,
                    TrustLevel::Verified,
                    Some(Utc::now()),
                )
                .await?;

            return Ok(Some(PublicDatasetMatch {
                public_dataset_name: public_dataset.name.clone(),
                public_dataset_version: public_dataset.version.clone(),
                verified_by: public_dataset.verified_by.clone(),
            }));
        }

        Ok(None)
    }

    /// Sign a dataset with user's key
    pub async fn sign_dataset(
        &self,
        dataset_id: Uuid,
        user_id: Uuid,
        private_key: &ed25519_dalek::SigningKey,
    ) -> Result<Vec<u8>, Error> {
        let dataset = self.state.metadata()
            .get_dataset(dataset_id)
            .await?;

        // Verify ownership
        if dataset.owner_id != user_id {
            return Err(Error::PermissionDenied);
        }

        // Sign the content hash
        let signature = private_key.sign(&dataset.content_hash);

        // Store signature
        self.state.metadata()
            .update_dataset_signature(dataset_id, signature.to_bytes().to_vec())
            .await?;

        Ok(signature.to_bytes().to_vec())
    }

    fn compute_manifest_hash(&self, files: &[DatasetFile]) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new();
        for file in files {
            hasher.update(file.path_in_dataset.as_bytes());
            hasher.update(&file.content_hash);
        }
        hasher.finalize().as_bytes().to_vec()
    }
}
```

### 4. Data Access Tokens

Short-lived tokens for Server Nodes to access data directly.

```rust
// cyxcloud-gateway/src/data_access.rs

pub struct DataAccessTokenService {
    state: Arc<AppState>,
    signing_key: ed25519_dalek::SigningKey,
}

impl DataAccessTokenService {
    /// Create a token for a Server Node to access a dataset
    pub async fn create_token(
        &self,
        request: CreateTokenRequest,
    ) -> Result<DataAccessToken, Error> {
        // Verify requester has access to dataset
        let dataset = self.state.metadata()
            .get_dataset(request.dataset_id)
            .await?;

        if dataset.owner_id != request.user_id {
            // Check if shared
            let shared = self.state.metadata()
                .check_dataset_access(request.dataset_id, request.user_id)
                .await?;
            if !shared {
                return Err(Error::PermissionDenied);
            }
        }

        // Create token
        let token_id = Uuid::new_v4();
        let expires_at = Utc::now() + chrono::Duration::hours(24);

        let token_data = TokenData {
            token_id,
            dataset_id: request.dataset_id,
            node_id: request.node_id,
            scopes: request.scopes,
            expires_at,
        };

        // Sign token
        let token_bytes = serde_json::to_vec(&token_data)?;
        let signature = self.signing_key.sign(&token_bytes);

        let token = DataAccessToken {
            data: token_data,
            signature: signature.to_bytes().to_vec(),
        };

        // Store token hash for revocation checking
        let token_hash = blake3::hash(&token_bytes);
        self.state.metadata()
            .store_data_access_token(
                token_id,
                request.dataset_id,
                request.node_id,
                token_hash.as_bytes().to_vec(),
                expires_at,
                request.scopes,
            )
            .await?;

        Ok(token)
    }

    /// Verify a token is valid
    pub async fn verify_token(&self, token: &DataAccessToken) -> Result<(), Error> {
        // Check signature
        let token_bytes = serde_json::to_vec(&token.data)?;
        let signature = ed25519_dalek::Signature::from_bytes(
            &token.signature.try_into().map_err(|_| Error::InvalidSignature)?
        );

        self.signing_key.verifying_key()
            .verify(&token_bytes, &signature)
            .map_err(|_| Error::InvalidSignature)?;

        // Check expiration
        if token.data.expires_at < Utc::now() {
            return Err(Error::TokenExpired);
        }

        // Check not revoked
        let token_hash = blake3::hash(&token_bytes);
        let stored = self.state.metadata()
            .get_data_access_token(token.data.token_id)
            .await?;

        if stored.is_none() {
            return Err(Error::TokenRevoked);
        }

        Ok(())
    }

    /// Revoke a token
    pub async fn revoke_token(&self, token_id: Uuid) -> Result<(), Error> {
        self.state.metadata()
            .delete_data_access_token(token_id)
            .await
    }
}
```

---

## Protocol Buffers

### DataStream Proto

```protobuf
// proto/datastream.proto

syntax = "proto3";

package cyxcloud.datastream;

service DataStreamService {
    // Stream batches for training
    rpc StreamBatches(StreamBatchesRequest) returns (stream BatchResponse);

    // Get dataset metadata
    rpc GetDatasetInfo(GetDatasetInfoRequest) returns (DatasetInfo);

    // Create access token for direct node access
    rpc CreateAccessToken(CreateAccessTokenRequest) returns (AccessToken);

    // Verify dataset integrity
    rpc VerifyDataset(VerifyDatasetRequest) returns (VerificationResult);
}

message StreamBatchesRequest {
    string dataset_id = 1;
    string access_token = 2;
    int32 batch_size = 3;
    int64 start_index = 4;
    int32 prefetch_batches = 5;  // How many batches to buffer
    int32 max_trust_level = 6;   // Maximum acceptable trust level
    bool shuffle = 7;
    int64 seed = 8;              // For reproducible shuffling
}

message BatchResponse {
    uint64 batch_index = 1;
    repeated bytes items = 2;          // Raw data for each item
    repeated bytes item_hashes = 3;    // Blake3 hash of each item
    bytes batch_hash = 4;              // Blake3 hash of entire batch
    uint64 total_batches = 5;
    bool is_last = 6;
}

message GetDatasetInfoRequest {
    string dataset_id = 1;
    string access_token = 2;
}

message DatasetInfo {
    string id = 1;
    string name = 2;
    string owner_id = 3;
    int64 total_size_bytes = 4;
    int32 file_count = 5;
    bytes content_hash = 6;
    TrustLevel trust_level = 7;
    bytes signature = 8;
    string schema_json = 9;
    int32 version = 10;
}

enum TrustLevel {
    TRUST_SELF = 0;
    TRUST_SIGNED = 1;
    TRUST_VERIFIED = 2;
    TRUST_ATTESTED = 3;
    TRUST_UNTRUSTED = 4;
}

message CreateAccessTokenRequest {
    string dataset_id = 1;
    string node_id = 2;
    repeated string scopes = 3;  // ["read", "stream"]
    int64 ttl_seconds = 4;
}

message AccessToken {
    string token = 1;
    int64 expires_at = 2;
    repeated string scopes = 3;
}

message VerifyDatasetRequest {
    string dataset_id = 1;
    bool check_public_registry = 2;
}

message VerificationResult {
    string dataset_id = 1;
    bool manifest_valid = 2;
    bool all_files_valid = 3;
    repeated FileVerification files = 4;
    PublicDatasetMatch public_match = 5;
    int64 verified_at = 6;
}

message FileVerification {
    string path = 1;
    string expected_hash = 2;
    string actual_hash = 3;
    bool valid = 4;
}

message PublicDatasetMatch {
    string name = 1;
    string version = 2;
    repeated string verified_by = 3;
}
```

---

## Database Schema

```sql
-- migrations/007_datastream.sql

-- ============================================================================
-- DATASETS TABLE
-- Versioned collections of files for ML training
-- ============================================================================

CREATE TABLE datasets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(256) NOT NULL,
    owner_id UUID NOT NULL,
    description TEXT,

    -- Content addressing
    content_hash BYTEA NOT NULL,          -- Blake3 of manifest
    total_size_bytes BIGINT NOT NULL,
    file_count INTEGER NOT NULL,

    -- Schema (for structured data)
    schema JSONB,

    -- Trust
    trust_level INTEGER NOT NULL DEFAULT 4,  -- 0=self, 4=untrusted
    signature BYTEA,                          -- Ed25519 signature
    verified_at TIMESTAMP WITH TIME ZONE,

    -- Versioning
    version INTEGER NOT NULL DEFAULT 1,
    parent_version_id UUID REFERENCES datasets(id),

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(owner_id, name, version)
);

CREATE INDEX idx_datasets_owner ON datasets(owner_id);
CREATE INDEX idx_datasets_content_hash ON datasets(content_hash);
CREATE INDEX idx_datasets_trust_level ON datasets(trust_level);

-- ============================================================================
-- DATASET_FILES TABLE
-- Files within a dataset
-- ============================================================================

CREATE TABLE dataset_files (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    file_id UUID NOT NULL REFERENCES files(id),
    path_in_dataset VARCHAR(1024) NOT NULL,
    content_hash BYTEA NOT NULL,
    size_bytes BIGINT NOT NULL,
    file_index INTEGER NOT NULL,

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(dataset_id, path_in_dataset)
);

CREATE INDEX idx_dataset_files_dataset ON dataset_files(dataset_id);
CREATE INDEX idx_dataset_files_file ON dataset_files(file_id);

-- ============================================================================
-- PUBLIC_DATASETS TABLE
-- Registry of known public datasets (ImageNet, CIFAR, etc.)
-- ============================================================================

CREATE TABLE public_datasets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(256) NOT NULL,
    version VARCHAR(64) NOT NULL,
    official_url TEXT NOT NULL,
    official_hash BYTEA NOT NULL,
    paper_url TEXT,
    license VARCHAR(256),

    -- CyxCloud cached copy
    cached_dataset_id UUID REFERENCES datasets(id),
    cached_at TIMESTAMP WITH TIME ZONE,

    -- Verification
    verified_by TEXT[] NOT NULL DEFAULT '{}',
    verified_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(name, version)
);

CREATE INDEX idx_public_datasets_hash ON public_datasets(official_hash);
CREATE INDEX idx_public_datasets_name ON public_datasets(name);

-- ============================================================================
-- DATA_ACCESS_TOKENS TABLE
-- Short-lived tokens for Server Node direct access
-- ============================================================================

CREATE TABLE data_access_tokens (
    id UUID PRIMARY KEY,
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    node_id UUID REFERENCES nodes(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL,
    scopes TEXT[] NOT NULL,
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    -- For cleanup
    revoked_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_data_access_tokens_dataset ON data_access_tokens(dataset_id);
CREATE INDEX idx_data_access_tokens_expires ON data_access_tokens(expires_at);

-- ============================================================================
-- DATASET_SHARES TABLE
-- Dataset sharing between users
-- ============================================================================

CREATE TABLE dataset_shares (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dataset_id UUID NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    shared_with_user_id UUID NOT NULL,
    permissions TEXT[] NOT NULL DEFAULT '{read}',  -- read, stream, reshare
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMP WITH TIME ZONE,

    UNIQUE(dataset_id, shared_with_user_id)
);

CREATE INDEX idx_dataset_shares_user ON dataset_shares(shared_with_user_id);

-- ============================================================================
-- VIEWS
-- ============================================================================

-- View: Dataset summary with trust info
CREATE VIEW dataset_summary AS
SELECT
    d.id,
    d.name,
    d.owner_id,
    d.total_size_bytes,
    d.file_count,
    d.trust_level,
    d.version,
    d.created_at,
    CASE d.trust_level
        WHEN 0 THEN 'self'
        WHEN 1 THEN 'signed'
        WHEN 2 THEN 'verified'
        WHEN 3 THEN 'attested'
        ELSE 'untrusted'
    END as trust_label,
    pd.name as public_dataset_name,
    pd.verified_by as public_verified_by
FROM datasets d
LEFT JOIN public_datasets pd ON pd.cached_dataset_id = d.id;
```

---

## Implementation Plan

### Phase 1: Database & Models (Week 1)

| Task | File | Description |
|------|------|-------------|
| 1.1 | `cyxcloud-metadata/migrations/007_datastream.sql` | Create schema |
| 1.2 | `cyxcloud-metadata/src/models.rs` | Add Dataset, DatasetFile, etc. |
| 1.3 | `cyxcloud-metadata/src/postgres.rs` | CRUD operations for datasets |
| 1.4 | `cyxcloud-metadata/src/lib.rs` | Export new types |

### Phase 2: Proto & gRPC (Week 1-2)

| Task | File | Description |
|------|------|-------------|
| 2.1 | `cyxcloud-protocol/proto/datastream.proto` | Define DataStream service |
| 2.2 | `cyxcloud-protocol/build.rs` | Generate Rust code |
| 2.3 | `cyxcloud-gateway/src/datastream.rs` | Implement DataStreamService |
| 2.4 | `cyxcloud-gateway/src/main.rs` | Register gRPC service |

### Phase 3: Verification (Week 2)

| Task | File | Description |
|------|------|-------------|
| 3.1 | `cyxcloud-gateway/src/verification.rs` | VerificationService |
| 3.2 | `cyxcloud-gateway/src/data_access.rs` | DataAccessTokenService |
| 3.3 | `cyxcloud-gateway/src/state.rs` | Add `get_object_verified()` |
| 3.4 | `cyxcloud-core/src/lib.rs` | Ed25519 signing utilities |

### Phase 4: Public Registry (Week 2-3)

| Task | File | Description |
|------|------|-------------|
| 4.1 | `cyxcloud-gateway/src/public_registry.rs` | PublicDatasetRegistry |
| 4.2 | `scripts/seed_public_datasets.sql` | Seed common datasets |
| 4.3 | `cyxcloud-cli/src/commands/dataset.rs` | CLI dataset commands |

### Phase 5: Server Node Integration (Week 3)

| Task | File | Description |
|------|------|-------------|
| 5.1 | `cyxcloud-node/src/datastream_client.rs` | DataStream gRPC client |
| 5.2 | `cyxcloud-node/src/data_loader.rs` | Streaming DataLoader |
| 5.3 | `cyxcloud-node/src/verification.rs` | Client-side verification |

### Phase 6: Testing (Week 3-4)

| Task | File | Description |
|------|------|-------------|
| 6.1 | `tests/datastream_test.rs` | Integration tests |
| 6.2 | `tests/verification_test.rs` | Hash verification tests |
| 6.3 | `tests/e2e_training_test.rs` | End-to-end training test |

### Phase 7: Engine GUI Integration (Week 4-5)

| Task | File | Description |
|------|------|-------------|
| 7.1 | `cyxwiz-engine/src/network/datastream_client.h/cpp` | DataStream gRPC client (C++) |
| 7.2 | `cyxwiz-engine/src/gui/panels/cloud_browser.h/cpp` | Browse CyxCloud datasets with search/filter |
| 7.3 | `cyxwiz-engine/src/gui/panels/cloud_dataset_manager.h/cpp` | Create, verify, share, delete datasets |
| 7.4 | `cyxwiz-engine/src/gui/panels/cloud_upload_dialog.h/cpp` | Upload local files to CyxCloud |
| 7.5 | `cyxwiz-engine/src/core/cloud_data_registry.h/cpp` | Integration with existing DataRegistry |
| 7.6 | `cyxwiz-engine/src/gui/panels/trust_badge.h/cpp` | Trust level visualization (icons, colors) |
| 7.7 | `cyxwiz-protocol/proto/datastream.proto` | Generate C++ stubs for Engine |

**Engine GUI Features:**

1. **Cloud Browser Panel** - Browse user's datasets and public registry
   - Tree view: My Datasets / Shared With Me / Public Datasets
   - Search by name, filter by trust level
   - Preview dataset info (size, file count, schema)
   - Double-click to load into DataRegistry

2. **Dataset Manager Panel** - Full dataset lifecycle
   - Create dataset from uploaded files
   - Verify dataset integrity
   - Share with other users (email/user ID)
   - View sharing permissions
   - Delete dataset

3. **Upload Dialog** - Upload local files to CyxCloud
   - Drag-and-drop folder/files
   - Progress bar with cancel
   - Auto-create dataset option

4. **Trust Badge Widget** - Visual trust indicators
   - Icons: 🔒 Self, ✍️ Signed, ✅ Verified, 🛡️ Attested, ⚠️ Untrusted
   - Tooltip with verification details
   - Click to verify/upgrade trust

5. **Node Editor Integration** - DataInput node enhancements
   - "Load from Cloud" button
   - Show trust badge on node
   - Streaming progress indicator

**Flow Diagram:**

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           CyxWiz Engine GUI                              │
│                                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌─────────────┐ │
│  │ Cloud        │  │ Dataset      │  │ Upload       │  │ Node Editor │ │
│  │ Browser      │  │ Manager      │  │ Dialog       │  │ (DataInput) │ │
│  │              │  │              │  │              │  │             │ │
│  │ ├─ My Data   │  │ [Create]     │  │ [Drop files] │  │ [Load from  │ │
│  │ ├─ Shared    │  │ [Verify]     │  │ [Upload]     │  │  Cloud]     │ │
│  │ └─ Public    │  │ [Share]      │  │ [Cancel]     │  │ ✅ CIFAR-10 │ │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬──────┘ │
│         │                 │                 │                 │         │
│         └─────────────────┴─────────────────┴─────────────────┘         │
│                                    │                                     │
│                         DataStreamClient (gRPC)                          │
└────────────────────────────────────┼─────────────────────────────────────┘
                                     │
                                     ▼
                            CyxCloud Gateway
```

---

## API Examples

### Create Dataset from Files

```bash
# Upload files
cyxcloud upload ./train_images/ --bucket datasets

# Create dataset from uploaded files
cyxcloud dataset create my-dataset \
    --files "datasets/train_images/**/*.jpg" \
    --schema '{"type": "image", "format": "jpg", "labels": "path"}'

# Output:
# Created dataset: my-dataset (version 1)
#   Files: 50000
#   Size: 2.3 GB
#   Hash: 7f83b1657ff1fc53b92dc18148a1d65d
#   Trust: self
```

### Verify Dataset

```bash
cyxcloud dataset verify my-dataset

# Output:
# Verifying dataset: my-dataset
#   Manifest hash: valid
#   Files verified: 50000/50000
#   Trust level: self
#
# Checking public registry...
#   Match found: CIFAR-10 (verified by torchvision, huggingface)
#   Trust level upgraded: self -> verified
```

### Create Access Token for Training

```bash
# Engine requests token for Server Node
cyxcloud dataset token my-dataset \
    --node node-1 \
    --scopes read,stream \
    --ttl 24h

# Output:
# Access token created:
#   Dataset: my-dataset
#   Node: node-1
#   Scopes: read, stream
#   Expires: 2026-01-04 12:00:00 UTC
#   Token: eyJ0eXAiOiJKV1QiLCJhbGciOiJFZDI1NTE5...
```

### Stream Data for Training (gRPC)

```python
# Server Node training code
import cyxcloud

# Connect to Gateway
client = cyxcloud.DataStreamClient("gateway:50052")

# Stream batches
for batch in client.stream_batches(
    dataset_id="my-dataset",
    access_token="eyJ0eXAiOiJKV1QiLCJhbGciOiJFZDI1NTE5...",
    batch_size=32,
    prefetch_batches=4,
    max_trust_level=2,  # Require at least "verified"
):
    # Batch is verified automatically
    images = batch.to_tensor()  # Direct to GPU
    labels = batch.labels

    # Train
    loss = model.train_step(images, labels)
    print(f"Batch {batch.index}: loss={loss:.4f}")
```

---

## Security Considerations

### 1. Token Security

- Tokens are signed with Ed25519 (gateway's key)
- Short TTL (default 24h)
- Scoped to specific dataset + node
- Revocable (stored hash in DB)

### 2. Data Integrity

- Every chunk verified against Blake3 hash
- Manifest hash covers all file hashes + paths
- Verification failure = abort training

### 3. Trust Model

- Default: untrusted (level 4)
- User's own data: self (level 0)
- Signed by known key: signed (level 1)
- Matches public registry: verified (level 2)
- Third-party attestation: attested (level 3)

### 4. Network Security

- All gRPC over mTLS (already implemented)
- Tokens not logged
- Data encrypted in transit

---

## Future Enhancements

1. **Differential Privacy** - Add noise during streaming
2. **Data Augmentation** - Server-side augmentation pipeline
3. **Caching** - Local cache on Server Nodes for repeated access
4. **Compression** - Compress batches during streaming
5. **Federated Registry** - Distributed public dataset registry
6. **Audit Log** - Track all data access for compliance
