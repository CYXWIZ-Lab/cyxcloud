//! Blockchain Types
//!
//! Shared types for CyxCloud blockchain integration.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Storage plan types matching the on-chain enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoragePlanType {
    Free,
    Starter,
    Pro,
    Enterprise,
}

impl StoragePlanType {
    /// Get storage quota in bytes
    pub fn storage_quota_bytes(&self) -> u64 {
        match self {
            StoragePlanType::Free => 5 * 1024 * 1024 * 1024, // 5 GB
            StoragePlanType::Starter => 100 * 1024 * 1024 * 1024, // 100 GB
            StoragePlanType::Pro => 1024 * 1024 * 1024 * 1024, // 1 TB
            StoragePlanType::Enterprise => 10 * 1024 * 1024 * 1024 * 1024, // 10 TB
        }
    }

    /// Get monthly price in CYXWIZ (9 decimals)
    pub fn monthly_price(&self) -> u64 {
        match self {
            StoragePlanType::Free => 0,
            StoragePlanType::Starter => 10_000_000_000, // 10 CYXWIZ
            StoragePlanType::Pro => 50_000_000_000,     // 50 CYXWIZ
            StoragePlanType::Enterprise => 200_000_000_000, // 200 CYXWIZ
        }
    }
}

/// Subscription period type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeriodType {
    Monthly,
    Yearly,
}

impl PeriodType {
    /// Get period duration in seconds
    pub fn duration_seconds(&self) -> i64 {
        match self {
            PeriodType::Monthly => 30 * 24 * 60 * 60, // 30 days
            PeriodType::Yearly => 365 * 24 * 60 * 60, // 365 days
        }
    }

    /// Get discount multiplier (yearly = 20% off)
    pub fn discount_multiplier(&self) -> f64 {
        match self {
            PeriodType::Monthly => 1.0,
            PeriodType::Yearly => 0.8, // 20% discount
        }
    }
}

/// Subscription status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionStatus {
    Active,
    Expired,
    Cancelled,
}

/// User subscription info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    pub user: Pubkey,
    pub plan_type: StoragePlanType,
    pub period_type: PeriodType,
    pub storage_quota_bytes: u64,
    pub storage_used_bytes: u64,
    pub bandwidth_used_bytes: u64,
    pub price_per_period: u64,
    pub current_period_start: i64,
    pub current_period_end: i64,
    pub auto_renew: bool,
    pub status: SubscriptionStatus,
}

/// Storage node status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageNodeStatus {
    Active,
    Inactive,
    Slashed,
}

/// Disk type for storage nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiskType {
    HDD,
    SSD,
    NVMe,
}

/// Storage node specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSpec {
    pub total_capacity_bytes: u64,
    pub allocated_capacity_bytes: u64,
    pub disk_type: DiskType,
    pub region: String,
    pub bandwidth_mbps: u32,
}

/// Storage node metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetrics {
    pub bytes_stored: u64,
    pub bytes_served: u64,
    pub uptime_seconds: u64,
    pub requests_served: u64,
}

/// Storage node reputation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageReputation {
    pub total_proofs: u64,
    pub failed_proofs: u64,
    pub data_loss_events: u32,
    pub retrieval_success_rate: u8, // 0-100
    pub slashes: u32,
    pub reputation_score: u16, // 0-10000
}

/// Storage node info from on-chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageNodeInfo {
    pub node_id: String,
    pub owner: Pubkey,
    pub stake_amount: u64,
    pub spec: StorageSpec,
    pub metrics: StorageMetrics,
    pub reputation: StorageReputation,
    pub status: StorageNodeStatus,
    pub last_heartbeat: i64,
    pub last_proof_timestamp: i64,
    pub pending_rewards: u64,
    pub registered_at: i64,
}

/// Epoch rewards info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRewardsInfo {
    pub epoch: u64,
    pub total_pool_amount: u64,
    pub nodes_share: u64,
    pub platform_share: u64,
    pub community_share: u64,
    pub allocated_to_nodes: u64,
    pub finalized: bool,
    pub platform_claimed: bool,
    pub community_claimed: bool,
    pub epoch_start: i64,
    pub epoch_end: i64,
}

/// Node epoch claim info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeEpochClaimInfo {
    pub epoch: u64,
    pub node_owner: Pubkey,
    pub node_id: String,
    pub reward_amount: u64,
    pub claimed: bool,
}

/// Payment pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentPoolInfo {
    pub authority: Pubkey,
    pub token_mint: Pubkey,
    pub current_epoch: u64,
    pub total_deposits: u64,
    pub node_percent: u8,
    pub platform_percent: u8,
    pub community_percent: u8,
    pub epoch_duration_seconds: i64,
    pub last_epoch_start: i64,
}

/// Blockchain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainConfig {
    /// Solana RPC URL
    pub rpc_url: String,

    /// Gateway authority keypair path
    pub keypair_path: Option<String>,

    /// CYXWIZ token mint address
    pub token_mint: Pubkey,

    /// StorageSubscription program ID
    pub subscription_program_id: Pubkey,

    /// StorageNodeRegistry program ID
    pub node_registry_program_id: Pubkey,

    /// StoragePaymentPool program ID
    pub payment_pool_program_id: Pubkey,

    /// Platform treasury token account
    pub platform_treasury: Pubkey,

    /// Community fund token account
    pub community_fund: Pubkey,
}

impl Default for BlockchainConfig {
    fn default() -> Self {
        use std::str::FromStr;

        // Default keypair path for CyxWiz ecosystem
        // Gateway authority: 4Y5HWB9W9SELq3Yoyf7mK7KF5kTbuaGxd2BMvn3AyAG8
        let default_keypair = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(|home| format!("{}/.config/solana/id.json", home))
            .ok();

        Self {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            keypair_path: default_keypair,

            // CYXWIZ Token (Devnet)
            token_mint: Pubkey::from_str("Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi")
                .expect("Invalid token mint"),

            // CyxCloud Storage Programs (Deployed 2025-12-22)
            subscription_program_id: Pubkey::from_str(
                "HZhWDJVkkUuHrgqkNb9bYxiCfqtnv8cnus9UQt843Fro",
            )
            .expect("Invalid subscription program ID"),
            node_registry_program_id: Pubkey::from_str(
                "AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z",
            )
            .expect("Invalid node registry program ID"),
            payment_pool_program_id: Pubkey::from_str(
                "4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX",
            )
            .expect("Invalid payment pool program ID"),

            // Platform treasury token account
            platform_treasury: Pubkey::from_str("negq5ApurkfM7V6F46NboJbnjbohEtfu1PotDsvMs5e")
                .expect("Invalid platform treasury"),

            // Community fund (TODO: create dedicated account)
            community_fund: Pubkey::from_str("negq5ApurkfM7V6F46NboJbnjbohEtfu1PotDsvMs5e")
                .expect("Invalid community fund"),
        }
    }
}

/// Constants for blockchain operations
pub mod constants {
    /// Minimum stake for storage nodes (500 CYXWIZ with 9 decimals)
    pub const MIN_STORAGE_STAKE: u64 = 500_000_000_000;

    /// Unstake lockup period (7 days in seconds)
    pub const UNSTAKE_LOCKUP_SECONDS: i64 = 604_800;

    /// Epoch duration (7 days in seconds)
    pub const EPOCH_DURATION_SECONDS: i64 = 604_800;

    /// Distribution percentages
    pub const STORAGE_NODE_PERCENT: u8 = 85;
    pub const PLATFORM_TREASURY_PERCENT: u8 = 10;
    pub const COMMUNITY_FUND_PERCENT: u8 = 5;

    /// Heartbeat timeout (5 minutes)
    pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 300;

    /// Slashing percentages
    pub const SLASH_DATA_LOSS: u8 = 10;
    pub const SLASH_EXTENDED_DOWNTIME: u8 = 5;
    pub const SLASH_CORRUPTED_DATA: u8 = 50;
    pub const SLASH_FAILED_PROOFS: u8 = 15;
}
