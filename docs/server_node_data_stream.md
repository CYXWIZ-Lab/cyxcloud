# Server Node DataStream Integration

This document explains how CyxCloud Server Nodes stream ML training data from the Gateway using the DataStream client modules implemented in Phase 5.

## Overview

When a Server Node executes an ML training job, it needs to fetch training data from CyxCloud's distributed storage. Rather than downloading entire datasets upfront, the DataStream system provides **zero-copy streaming** of training batches with cryptographic verification.

```
┌─────────────────┐                    ┌─────────────────┐
│   Server Node   │                    │     Gateway     │
│                 │                    │                 │
│ ┌─────────────┐ │    gRPC Stream     │ ┌─────────────┐ │
│ │  Training   │ │◄──────────────────►│ │ DataStream  │ │
│ │  Executor   │ │   StreamBatches    │ │   Service   │ │
│ └──────┬──────┘ │                    │ └──────┬──────┘ │
│        │        │                    │        │        │
│ ┌──────▼──────┐ │                    │ ┌──────▼──────┐ │
│ │ DataLoader  │ │                    │ │  Metadata   │ │
│ │ (prefetch)  │ │                    │ │  + Storage  │ │
│ └──────┬──────┘ │                    │ └─────────────┘ │
│        │        │                    │                 │
│ ┌──────▼──────┐ │                    │                 │
│ │ Verification│ │                    │                 │
│ │ (Blake3)    │ │                    │                 │
│ └─────────────┘ │                    │                 │
└─────────────────┘                    └─────────────────┘
```

## Components

### 1. DataStreamClient (`datastream_client.rs`)

The low-level gRPC client that communicates with the Gateway's DataStreamService.

**Key Features:**
- TLS-enabled connections
- Access token authentication
- Blake3 hash verification per batch
- Configurable trust level filtering

```rust
use cyxcloud_node::{DataStreamClient, DataStreamConfig};

// Create client
let config = DataStreamConfig {
    gateway_addr: "https://gateway:50052".to_string(),
    access_token: Some("token".to_string()),
    batch_size: 32,
    prefetch_batches: 4,
    max_trust_level: 2, // Verified or better
    ..Default::default()
};

let mut client = DataStreamClient::connect(config).await?;

// Get dataset info
let info = client.get_dataset_info("dataset-uuid").await?;

// Stream batches
let mut rx = client.stream_batches("dataset-uuid", 0, true, Some(42)).await?;
while let Some(batch) = rx.recv().await {
    let verified_batch = batch?;
    // Process verified_batch.items
}
```

**Batch Verification Flow:**

```
Gateway sends BatchResponse:
┌────────────────────────────────────┐
│ batch_index: 5                     │
│ items: [data1, data2, data3, ...]  │
│ item_hashes: [h1, h2, h3, ...]     │
│ batch_hash: H                      │
│ total_batches: 100                 │
│ is_last: false                     │
└────────────────────────────────────┘
                 │
                 ▼
┌────────────────────────────────────┐
│ verify_batch():                    │
│                                    │
│ 1. For each item[i]:               │
│    assert blake3(item[i]) == h[i]  │
│                                    │
│ 2. Compute batch hash:             │
│    H' = blake3(item1 || item2 ...) │
│    assert H' == H                  │
│                                    │
│ 3. Return VerifiedBatch            │
└────────────────────────────────────┘
```

### 2. DataLoader (`data_loader.rs`)

A PyTorch-style DataLoader that wraps the DataStreamClient for convenient training loops.

**Key Features:**
- Multi-epoch iteration
- Automatic shuffling per epoch
- Background prefetching
- Pause/resume/stop controls
- Statistics tracking

```rust
use cyxcloud_node::{DataLoaderBuilder, LoaderState};

let loader = DataLoaderBuilder::new("dataset-uuid")
    .batch_size(64)
    .prefetch(8)
    .shuffle(true)
    .seed(42)
    .epochs(10)
    .gateway_addr("https://gateway:50052")
    .access_token("token")
    .build();

// Start streaming
let mut rx = loader.start().await?;

while let Some(batch_result) = rx.recv().await {
    let batch = batch_result?;

    println!(
        "Epoch {}/{}, Batch {}/{}",
        batch.epoch,
        10,
        batch.epoch_index,
        batch.total_batches
    );

    // Process batch.items
    for item in &batch.items {
        // item is Vec<u8> - raw training sample
    }

    if batch.is_epoch_end {
        println!("Epoch {} complete!", batch.epoch);
    }
}
```

**DataLoader State Machine:**

```
     ┌──────┐
     │ Idle │
     └──┬───┘
        │ start()
        ▼
   ┌─────────┐
   │ Loading │◄────────────┐
   └────┬────┘             │
        │                  │ resume()
   pause()                 │
        │              ┌───┴───┐
        └─────────────►│Paused │
                       └───┬───┘
                           │ stop()
        ┌──────────────────┤
        ▼                  ▼
┌──────────────┐    ┌─────────┐
│EpochComplete │    │ Stopped │
└──────┬───────┘    └─────────┘
       │ (next epoch or done)
       ▼
  ┌─────────┐
  │ Stopped │
  └─────────┘
```

### 3. Verification Module (`verification.rs`)

Handles trust level requirements and dataset verification before training.

**Trust Levels:**

| Level | Name | Description |
|-------|------|-------------|
| 0 | Self | User's own uploads |
| 1 | Signed | Cryptographically signed by trusted party |
| 2 | Verified | Hash matches public registry (MNIST, CIFAR, etc.) |
| 3 | Attested | TEE/SGX attested |
| 4 | Untrusted | Unknown source |

**Trust Requirements:**

```rust
use cyxcloud_node::{DatasetVerifier, TrustRequirement};

// Require dataset to be verified against public registry
let verifier = DatasetVerifier::new()
    .trust_requirement(TrustRequirement::VerifiedOrBetter)
    .check_public_registry(true)
    .full_verification(false);

let result = verifier.verify_dataset(&mut client, "dataset-uuid").await?;

if result.passed {
    println!("Trust level: {}", result.trust_level_name);
    if result.is_public_dataset {
        println!("Matches: {} v{}",
            result.public_dataset_name.unwrap(),
            result.public_dataset_version.unwrap()
        );
    }
}
```

**Trust Requirement Satisfaction:**

```
TrustRequirement::Any           → accepts levels 0,1,2,3,4
TrustRequirement::SelfOrBetter  → accepts levels 0 only
TrustRequirement::SignedOrBetter → accepts levels 0,1
TrustRequirement::VerifiedOrBetter → accepts levels 0,1,2
TrustRequirement::AttestedOnly  → accepts levels 0,1,2,3
```

### 4. TrainingExecutor (`training_executor.rs`)

High-level job executor that integrates all components for end-to-end training.

**Key Features:**
- Pre-training dataset verification
- Automatic DataLoader setup
- Status callbacks for monitoring
- Batch processing callbacks
- Full lifecycle management

```rust
use cyxcloud_node::{TrainingExecutorBuilder, TrustRequirement, TrainingState};

let mut executor = TrainingExecutorBuilder::new("dataset-uuid")
    .batch_size(32)
    .epochs(10)
    .shuffle(true)
    .seed(42)
    .trust_requirement(TrustRequirement::VerifiedOrBetter)
    .verify_before_training(true)
    .gateway_addr("https://gateway:50052")
    .access_token("token")
    .build();

// Set callback to process each batch
executor.set_batch_callback(|batch| {
    // Your ML training code here
    // batch.items contains the raw training data
    println!("Processing batch {} with {} items",
        batch.epoch_index,
        batch.items.len()
    );
    Ok(())
});

// Set callback for status updates
executor.set_status_callback(|status| {
    println!(
        "Progress: Epoch {}/{}, Batch {}/{}, ETA: {:?}",
        status.epoch,
        status.total_epochs,
        status.epoch_batches,
        status.total_batches,
        status.eta
    );
});

// Run training
let final_status = executor.run().await?;

match final_status.state {
    TrainingState::Completed => println!("Training finished!"),
    TrainingState::Failed => println!("Error: {:?}", final_status.error),
    TrainingState::Stopped => println!("Training was stopped"),
    _ => {}
}
```

**Training Execution Flow:**

```
executor.run()
      │
      ▼
┌─────────────────────┐
│ 1. VERIFYING        │
│    - Connect to GW  │
│    - Check trust    │
│    - Validate hash  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ 2. INITIALIZING     │
│    - Create loader  │
│    - Start stream   │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ 3. TRAINING         │◄──────┐
│    - Receive batch  │       │
│    - Verify hash    │       │
│    - Call callback  │       │
│    - Update stats   │───────┘
└──────────┬──────────┘  (next batch)
           │
           │ (all epochs done)
           ▼
┌─────────────────────┐
│ 4. COMPLETED        │
│    - Return status  │
└─────────────────────┘
```

## Configuration Reference

### DataStreamConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gateway_addr` | String | `http://localhost:50052` | Gateway gRPC address |
| `access_token` | Option<String> | None | Authentication token |
| `batch_size` | i32 | 32 | Items per batch |
| `prefetch_batches` | i32 | 4 | Batches to prefetch |
| `max_trust_level` | i32 | 2 (Verified) | Maximum acceptable trust |
| `tls_config` | Option<TlsClientConfig> | None | TLS settings |
| `connect_timeout_secs` | u64 | 30 | Connection timeout |

### DataLoaderConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dataset_id` | String | - | Dataset UUID |
| `batch_size` | i32 | 32 | Items per batch |
| `prefetch_batches` | i32 | 4 | Prefetch count |
| `shuffle` | bool | true | Shuffle each epoch |
| `seed` | Option<i64> | None | Random seed |
| `drop_last` | bool | false | Drop incomplete last batch |
| `num_epochs` | Option<u32> | Some(1) | Epochs (None = infinite) |

### TrainingJobConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dataset_id` | String | - | Dataset UUID |
| `batch_size` | i32 | 32 | Items per batch |
| `epochs` | u32 | 1 | Training epochs |
| `shuffle` | bool | true | Shuffle data |
| `seed` | Option<i64> | None | Random seed |
| `trust_requirement` | TrustRequirement | VerifiedOrBetter | Min trust level |
| `verify_before_training` | bool | true | Pre-verify dataset |

## Environment Variables

The DataStreamConfig can be configured via environment:

| Variable | Description |
|----------|-------------|
| `GATEWAY_ADDR` | Gateway address |
| `DATASTREAM_ACCESS_TOKEN` | Auth token |
| `DATASTREAM_BATCH_SIZE` | Batch size |
| `DATASTREAM_PREFETCH_BATCHES` | Prefetch count |
| `DATASTREAM_MAX_TRUST_LEVEL` | Max trust level |

```rust
let config = DataStreamConfig::from_env();
```

## Error Handling

### DataStreamError

| Variant | Cause |
|---------|-------|
| `ConnectionFailed` | Cannot connect to Gateway |
| `GrpcError` | gRPC call failed |
| `Transport` | Network transport error |
| `DatasetNotFound` | Dataset UUID not found |
| `AccessDenied` | Invalid/expired token |
| `TokenExpired` | Access token expired |
| `HashMismatch` | Batch verification failed |
| `TrustLevelExceeded` | Dataset trust too low |
| `StreamEnded` | Unexpected stream termination |

### TrainingError

| Variant | Cause |
|---------|-------|
| `VerificationFailed` | Dataset verification failed |
| `LoadingError` | Data streaming error |
| `Interrupted` | Training stopped by user |
| `ConfigError` | Invalid configuration |
| `CallbackError` | Batch callback returned error |

## Security Considerations

1. **Hash Verification**: Every batch is verified using Blake3 hashes before processing. This prevents data tampering during transit.

2. **Trust Levels**: Training jobs can enforce minimum trust requirements, ensuring only verified datasets are used.

3. **Access Tokens**: Short-lived tokens authenticate Server Nodes to the Gateway. Tokens are scoped to specific datasets.

4. **TLS**: All Gateway connections support TLS with client certificates for mutual authentication.

## Performance Tips

1. **Prefetch Tuning**: Increase `prefetch_batches` for high-latency connections to hide network latency.

2. **Batch Size**: Larger batches reduce overhead but increase memory usage. Match to GPU memory.

3. **Parallel Processing**: The DataLoader streams batches in a background task. Process batches as fast as possible to keep the pipeline full.

4. **Shuffling**: Enable shuffling for better training convergence. The Gateway handles shuffling server-side, so there's no client overhead.
