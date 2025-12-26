//! CyxCloud Blockchain Integration
//!
//! This module provides integration with Solana blockchain for:
//! - Storage subscriptions (user plans and billing)
//! - Storage node registry (node staking and proof-of-storage)
//! - Payment pool (epoch-based reward distribution)
//!
//! # Architecture
//!
//! The Gateway acts as the coordinating authority for on-chain operations:
//! - Updates user storage usage
//! - Manages payment epochs (weekly)
//! - Distributes rewards to storage nodes
//! - Slashes misbehaving nodes
//!
//! # Programs
//!
//! - `StorageSubscription`: User storage plans (Free/Starter/Pro/Enterprise)
//! - `StorageNodeRegistry`: Node registration, staking, and proof-of-storage
//! - `StoragePaymentPool`: Payment accumulation and 85/10/5 distribution
//!
//! # Example
//!
//! ```rust,ignore
//! use cyxcloud_gateway::blockchain::{CyxCloudBlockchainClient, BlockchainConfig};
//!
//! // Create client from environment
//! let client = CyxCloudBlockchainClient::from_env()?;
//!
//! // Check user subscription
//! let subscription = client.get_subscription(&user_pubkey).await?;
//!
//! // Update usage
//! client.update_usage(&user_pubkey, storage_bytes, bandwidth_bytes).await?;
//!
//! // Check if epoch should end
//! if client.should_end_epoch().await? {
//!     client.start_new_epoch().await?;
//! }
//! ```

mod client;
mod node_registry;
mod payment_pool;
mod subscription;
pub mod types;

// Re-export main types
pub use client::CyxCloudBlockchainClient;
pub use types::{
    BlockchainConfig,
    DiskType,
    EpochRewardsInfo,
    NodeEpochClaimInfo,
    PaymentPoolInfo,
    PeriodType,
    StorageMetrics,
    StorageNodeInfo,
    StorageNodeStatus,
    StoragePlanType,
    StorageReputation,
    StorageSpec,
    SubscriptionInfo,
    SubscriptionStatus,
};

// Re-export constants
pub use types::constants;

// Re-export operations for advanced use cases
pub use node_registry::NodeRegistryOps;
pub use payment_pool::{NodeWeight, PaymentPoolOps};
pub use subscription::SubscriptionOps;
