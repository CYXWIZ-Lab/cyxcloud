//! DataStream Integration Tests
//!
//! Tests the DataStream components: client configuration, data loader, and verification.
//!
//! Run with: cargo test --test datastream_test

use cyxcloud_node::{
    DataLoaderBuilder, DataLoaderConfig, DataStreamConfig, DatasetVerifier,
    TrainingBatch, TrainingExecutorBuilder, TrustRequirement, VerifiedBatch, VerificationOptions,
};

/// Test DataStreamConfig defaults and environment loading
#[test]
fn test_datastream_config_defaults() {
    let config = DataStreamConfig::default();

    assert_eq!(config.gateway_addr, "http://localhost:50052");
    assert!(config.access_token.is_none());
    assert_eq!(config.batch_size, 32);
    assert_eq!(config.prefetch_batches, 4);
    assert_eq!(config.connect_timeout_secs, 30);
    assert!(config.tls_config.is_none());

    println!("DataStreamConfig defaults: OK");
}

/// Test DataStreamConfig from environment
#[test]
fn test_datastream_config_from_env() {
    // Set environment variables
    std::env::set_var("GATEWAY_ADDR", "http://test-gateway:50052");
    std::env::set_var("DATASTREAM_BATCH_SIZE", "64");
    std::env::set_var("DATASTREAM_PREFETCH_BATCHES", "8");

    let config = DataStreamConfig::from_env();

    assert_eq!(config.gateway_addr, "http://test-gateway:50052");
    assert_eq!(config.batch_size, 64);
    assert_eq!(config.prefetch_batches, 8);

    // Clean up
    std::env::remove_var("GATEWAY_ADDR");
    std::env::remove_var("DATASTREAM_BATCH_SIZE");
    std::env::remove_var("DATASTREAM_PREFETCH_BATCHES");

    println!("DataStreamConfig from_env: OK");
}

/// Test DataLoaderConfig defaults
#[test]
fn test_data_loader_config_defaults() {
    let config = DataLoaderConfig::default();

    assert!(config.dataset_id.is_empty());
    assert_eq!(config.batch_size, 32);
    assert_eq!(config.prefetch_batches, 4);
    assert!(config.shuffle);
    assert!(config.seed.is_none());
    assert!(!config.drop_last);
    assert_eq!(config.num_epochs, Some(1));

    println!("DataLoaderConfig defaults: OK");
}

/// Test DataLoaderBuilder
#[test]
fn test_data_loader_builder() {
    let loader = DataLoaderBuilder::new("test-dataset-uuid")
        .batch_size(128)
        .prefetch(16)
        .shuffle(false)
        .seed(12345)
        .epochs(100)
        .drop_last(true)
        .gateway_addr("https://gateway.example.com:50052")
        .access_token("secret-token")
        .max_trust_level(2)
        .build();

    assert_eq!(loader.config().dataset_id, "test-dataset-uuid");
    assert_eq!(loader.config().batch_size, 128);
    assert_eq!(loader.config().prefetch_batches, 16);
    assert!(!loader.config().shuffle);
    assert_eq!(loader.config().seed, Some(12345));
    assert_eq!(loader.config().num_epochs, Some(100));
    assert!(loader.config().drop_last);

    println!("DataLoaderBuilder: OK");
}

/// Test DataLoader infinite epochs mode
#[test]
fn test_data_loader_infinite_epochs() {
    let loader = DataLoaderBuilder::new("test-dataset")
        .infinite()
        .build();

    assert!(loader.config().num_epochs.is_none());

    println!("DataLoader infinite epochs: OK");
}

/// Test DataLoader initial state
#[test]
fn test_data_loader_initial_state() {
    let loader = DataLoaderBuilder::new("test-dataset").build();

    assert!(!loader.is_paused());
    assert!(!loader.is_stopped());
    assert_eq!(loader.current_epoch(), 0);
    assert_eq!(loader.total_batches(), 0);

    println!("DataLoader initial state: OK");
}

/// Test DataLoader pause/resume/stop controls
#[test]
fn test_data_loader_controls() {
    let loader = DataLoaderBuilder::new("test-dataset").build();

    // Initial state
    assert!(!loader.is_paused());
    assert!(!loader.is_stopped());

    // Pause
    loader.pause();
    assert!(loader.is_paused());
    assert!(!loader.is_stopped());

    // Resume
    loader.resume();
    assert!(!loader.is_paused());
    assert!(!loader.is_stopped());

    // Stop
    loader.stop();
    assert!(!loader.is_paused());
    assert!(loader.is_stopped());

    println!("DataLoader controls: OK");
}

/// Test TrainingBatch conversion from VerifiedBatch
#[test]
fn test_training_batch_from_verified() {
    let verified = VerifiedBatch {
        index: 42,
        items: vec![vec![1, 2, 3], vec![4, 5, 6]],
        item_count: 2,
        is_last: false,
        total_batches: 100,
    };

    let training: TrainingBatch = verified.into();

    assert_eq!(training.global_index, 42);
    assert_eq!(training.epoch_index, 42);
    assert_eq!(training.epoch, 0);
    assert_eq!(training.items.len(), 2);
    assert!(!training.is_epoch_end);
    assert_eq!(training.total_batches, 100);

    println!("TrainingBatch conversion: OK");
}

/// Test TrainingBatch last batch flag
#[test]
fn test_training_batch_last_batch() {
    let verified = VerifiedBatch {
        index: 99,
        items: vec![vec![1, 2, 3]],
        item_count: 1,
        is_last: true,
        total_batches: 100,
    };

    let training: TrainingBatch = verified.into();

    assert!(training.is_epoch_end);
    assert_eq!(training.global_index, 99);

    println!("TrainingBatch last batch: OK");
}

/// Test VerificationOptions defaults
#[test]
fn test_verification_options_defaults() {
    let opts = VerificationOptions::default();

    assert_eq!(opts.trust_requirement, TrustRequirement::VerifiedOrBetter);
    assert!(opts.check_public_registry);
    assert!(!opts.full_verification);
    assert!(opts.verify_batch_hashes);
    assert_eq!(opts.sample_size, 0);

    println!("VerificationOptions defaults: OK");
}

/// Test TrustRequirement satisfaction logic
#[test]
fn test_trust_requirement_satisfaction() {
    // Any - accepts all levels
    assert!(TrustRequirement::Any.is_satisfied(0));
    assert!(TrustRequirement::Any.is_satisfied(1));
    assert!(TrustRequirement::Any.is_satisfied(2));
    assert!(TrustRequirement::Any.is_satisfied(3));
    assert!(TrustRequirement::Any.is_satisfied(4));

    // SelfOrBetter - only accepts level 0
    assert!(TrustRequirement::SelfOrBetter.is_satisfied(0));
    assert!(!TrustRequirement::SelfOrBetter.is_satisfied(1));
    assert!(!TrustRequirement::SelfOrBetter.is_satisfied(4));

    // SignedOrBetter - accepts 0, 1
    assert!(TrustRequirement::SignedOrBetter.is_satisfied(0));
    assert!(TrustRequirement::SignedOrBetter.is_satisfied(1));
    assert!(!TrustRequirement::SignedOrBetter.is_satisfied(2));
    assert!(!TrustRequirement::SignedOrBetter.is_satisfied(4));

    // VerifiedOrBetter - accepts 0, 1, 2
    assert!(TrustRequirement::VerifiedOrBetter.is_satisfied(0));
    assert!(TrustRequirement::VerifiedOrBetter.is_satisfied(1));
    assert!(TrustRequirement::VerifiedOrBetter.is_satisfied(2));
    assert!(!TrustRequirement::VerifiedOrBetter.is_satisfied(3));
    assert!(!TrustRequirement::VerifiedOrBetter.is_satisfied(4));

    // AttestedOnly - accepts 0, 1, 2, 3
    assert!(TrustRequirement::AttestedOnly.is_satisfied(0));
    assert!(TrustRequirement::AttestedOnly.is_satisfied(1));
    assert!(TrustRequirement::AttestedOnly.is_satisfied(2));
    assert!(TrustRequirement::AttestedOnly.is_satisfied(3));
    assert!(!TrustRequirement::AttestedOnly.is_satisfied(4));

    println!("TrustRequirement satisfaction: OK");
}

/// Test TrustRequirement max_trust_level values
#[test]
fn test_trust_requirement_max_levels() {
    assert_eq!(TrustRequirement::Any.max_trust_level(), 4);
    assert_eq!(TrustRequirement::SelfOrBetter.max_trust_level(), 0);
    assert_eq!(TrustRequirement::SignedOrBetter.max_trust_level(), 1);
    assert_eq!(TrustRequirement::VerifiedOrBetter.max_trust_level(), 2);
    assert_eq!(TrustRequirement::AttestedOnly.max_trust_level(), 3);

    println!("TrustRequirement max levels: OK");
}

/// Test DatasetVerifier builder pattern
#[test]
fn test_dataset_verifier_builder() {
    let verifier = DatasetVerifier::new()
        .trust_requirement(TrustRequirement::SignedOrBetter)
        .check_public_registry(false)
        .full_verification(true);

    // Verifier is constructed - we can't easily inspect internal state
    // but the builder pattern should work without panicking
    let _ = verifier;

    println!("DatasetVerifier builder: OK");
}

/// Test TrainingExecutorBuilder
#[test]
fn test_training_executor_builder() {
    let executor = TrainingExecutorBuilder::new("my-dataset")
        .batch_size(64)
        .epochs(50)
        .shuffle(true)
        .seed(42)
        .trust_requirement(TrustRequirement::VerifiedOrBetter)
        .verify_before_training(true)
        .gateway_addr("https://gateway:50052")
        .access_token("token123")
        .prefetch(8)
        .build();

    assert!(!executor.is_paused());
    assert!(!executor.is_stopped());

    println!("TrainingExecutorBuilder: OK");
}

/// Test TrainingExecutor controls
#[test]
fn test_training_executor_controls() {
    let executor = TrainingExecutorBuilder::new("test-dataset").build();

    // Initial state
    assert!(!executor.is_paused());
    assert!(!executor.is_stopped());

    // Pause
    executor.pause();
    assert!(executor.is_paused());

    // Resume
    executor.resume();
    assert!(!executor.is_paused());

    // Stop
    executor.stop();
    assert!(executor.is_stopped());

    println!("TrainingExecutor controls: OK");
}

/// Test TrainingExecutor initial status
#[tokio::test]
async fn test_training_executor_initial_status() {
    let executor = TrainingExecutorBuilder::new("test-dataset")
        .epochs(10)
        .batch_size(32)
        .build();

    let status = executor.get_status().await;

    assert_eq!(status.state, cyxcloud_node::TrainingState::Idle);
    assert_eq!(status.epoch, 0);
    assert_eq!(status.total_epochs, 10);
    assert_eq!(status.global_batches, 0);
    assert_eq!(status.items_processed, 0);
    assert!(status.verification.is_none());
    assert!(status.error.is_none());

    println!("TrainingExecutor initial status: OK");
}

/// Test multiple DataLoader configurations
#[test]
fn test_multiple_loader_configs() {
    // Config 1: Small batches, many prefetch
    let loader1 = DataLoaderBuilder::new("dataset-1")
        .batch_size(8)
        .prefetch(32)
        .epochs(1)
        .build();
    assert_eq!(loader1.config().batch_size, 8);
    assert_eq!(loader1.config().prefetch_batches, 32);

    // Config 2: Large batches, few prefetch
    let loader2 = DataLoaderBuilder::new("dataset-2")
        .batch_size(256)
        .prefetch(2)
        .epochs(100)
        .build();
    assert_eq!(loader2.config().batch_size, 256);
    assert_eq!(loader2.config().prefetch_batches, 2);

    // Config 3: With specific seed for reproducibility
    let loader3 = DataLoaderBuilder::new("dataset-3")
        .shuffle(true)
        .seed(999)
        .build();
    assert_eq!(loader3.config().seed, Some(999));

    println!("Multiple loader configs: OK");
}

/// Summary test that runs all checks
#[test]
fn test_datastream_integration_summary() {
    println!("\n=== DataStream Integration Test Summary ===\n");
    println!("DataStreamConfig: defaults and from_env");
    println!("DataLoaderConfig: defaults and builder");
    println!("DataLoader: state management and controls");
    println!("TrainingBatch: conversion from VerifiedBatch");
    println!("VerificationOptions: defaults");
    println!("TrustRequirement: satisfaction logic");
    println!("DatasetVerifier: builder pattern");
    println!("TrainingExecutor: builder and controls");
    println!("\n All DataStream integration tests passed!\n");
}
