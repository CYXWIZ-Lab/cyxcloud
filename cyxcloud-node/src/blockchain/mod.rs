//! Storage Node Blockchain Integration
//!
//! This module provides Solana blockchain integration for storage nodes:
//! - Node registration and staking
//! - Heartbeat with proof-of-storage
//! - Reward claiming for epochs
//!
//! # Architecture
//!
//! Storage nodes interact with two main programs:
//! - `StorageNodeRegistry`: Registration, staking, heartbeat, proof-of-storage
//! - `StoragePaymentPool`: Claim epoch rewards
//!
//! # Example
//!
//! ```rust,ignore
//! use cyxcloud_node::blockchain::{StorageNodeBlockchainClient, NodeBlockchainConfig};
//!
//! // Create client from environment
//! let client = StorageNodeBlockchainClient::from_env()?;
//!
//! // Register node (if not already registered)
//! if !client.is_registered().await? {
//!     client.register_node("my-node-001", 500_000_000_000, storage_spec).await?;
//! }
//!
//! // Send heartbeat with storage proof
//! client.heartbeat().await?;
//!
//! // Claim rewards for previous epoch
//! client.claim_rewards(epoch).await?;
//! ```

mod client;
pub mod heartbeat;
mod registration;
mod rewards;
mod types;

// Re-export main client
pub use client::StorageNodeBlockchainClient;

// Re-export types
pub use types::{
    DiskType, NodeBlockchainConfig, ProofChallenge, ProofOfStorage, StorageNodeStatus, StorageSpec,
};

// Re-export operations for advanced use
pub use heartbeat::HeartbeatOps;
pub use registration::RegistrationOps;
pub use rewards::RewardsOps;

// Re-export constants
pub use types::constants;
