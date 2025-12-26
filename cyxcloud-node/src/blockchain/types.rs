//! Blockchain Types for Storage Node
//!
//! Configuration and data types for blockchain integration.

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Storage node status on-chain
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageNodeStatus {
    Active,
    Inactive,
    Slashed,
}

impl StorageNodeStatus {
    /// Parse from on-chain byte value
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => StorageNodeStatus::Active,
            1 => StorageNodeStatus::Inactive,
            _ => StorageNodeStatus::Slashed,
        }
    }

    /// Convert to on-chain byte value
    pub fn to_byte(&self) -> u8 {
        match self {
            StorageNodeStatus::Active => 0,
            StorageNodeStatus::Inactive => 1,
            StorageNodeStatus::Slashed => 2,
        }
    }
}

/// Disk type for storage specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiskType {
    HDD,
    SSD,
    NVMe,
}

impl DiskType {
    /// Parse from on-chain byte value
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => DiskType::HDD,
            1 => DiskType::SSD,
            _ => DiskType::NVMe,
        }
    }

    /// Convert to on-chain byte value
    pub fn to_byte(&self) -> u8 {
        match self {
            DiskType::HDD => 0,
            DiskType::SSD => 1,
            DiskType::NVMe => 2,
        }
    }
}

/// Storage specification for node registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSpec {
    /// Total storage capacity in bytes
    pub total_capacity_bytes: u64,

    /// Disk type (HDD, SSD, NVMe)
    pub disk_type: DiskType,

    /// Geographic region (e.g., "us-east-1")
    pub region: String,

    /// Network bandwidth in Mbps
    pub bandwidth_mbps: u32,
}

impl StorageSpec {
    /// Create a new storage specification
    pub fn new(
        total_capacity_bytes: u64,
        disk_type: DiskType,
        region: impl Into<String>,
        bandwidth_mbps: u32,
    ) -> Self {
        Self {
            total_capacity_bytes,
            disk_type,
            region: region.into(),
            bandwidth_mbps,
        }
    }
}

/// Proof-of-storage challenge from Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofChallenge {
    /// Challenge ID (unique per challenge)
    pub challenge_id: u64,

    /// Chunk ID to prove storage of
    pub chunk_id: String,

    /// Random nonce for this challenge
    pub nonce: [u8; 32],

    /// Timestamp when challenge was issued
    pub timestamp: i64,

    /// Deadline for submitting proof
    pub deadline: i64,
}

/// Proof-of-storage response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfStorage {
    /// Challenge ID being responded to
    pub challenge_id: u64,

    /// Hash of (chunk_data || nonce)
    pub proof_hash: [u8; 32],

    /// Optional merkle path for partial chunk verification
    pub merkle_path: Option<Vec<[u8; 32]>>,
}

/// Node information from on-chain account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageNodeInfo {
    pub node_id: String,
    pub owner: Pubkey,
    pub stake_amount: u64,
    pub total_capacity_bytes: u64,
    pub allocated_capacity_bytes: u64,
    pub disk_type: DiskType,
    pub region: String,
    pub bandwidth_mbps: u32,
    pub status: StorageNodeStatus,
    pub last_heartbeat: i64,
    pub last_proof_timestamp: i64,
    pub total_proofs: u64,
    pub failed_proofs: u64,
    pub reputation_score: u16,
    pub pending_rewards: u64,
    pub registered_at: i64,
}

/// Epoch claim information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochClaimInfo {
    pub epoch: u64,
    pub reward_amount: u64,
    pub claimed: bool,
}

/// Blockchain configuration for storage node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBlockchainConfig {
    /// Solana RPC URL
    pub rpc_url: String,

    /// Node owner keypair path
    pub keypair_path: Option<String>,

    /// CYXWIZ token mint address
    pub token_mint: Pubkey,

    /// StorageNodeRegistry program ID
    pub node_registry_program_id: Pubkey,

    /// StoragePaymentPool program ID
    pub payment_pool_program_id: Pubkey,
}

impl Default for NodeBlockchainConfig {
    fn default() -> Self {
        use std::str::FromStr;

        // Default keypair path
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
            node_registry_program_id: Pubkey::from_str(
                "AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z",
            )
            .expect("Invalid node registry program ID"),

            payment_pool_program_id: Pubkey::from_str(
                "4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX",
            )
            .expect("Invalid payment pool program ID"),
        }
    }
}

impl NodeBlockchainConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> anyhow::Result<Self> {
        use std::str::FromStr;

        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

        let keypair_path = std::env::var("NODE_KEYPAIR_PATH").ok();

        let token_mint = std::env::var("CYXWIZ_TOKEN_MINT")
            .ok()
            .map(|s| Pubkey::from_str(&s))
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid token mint: {}", e))?
            .unwrap_or_else(|| {
                Pubkey::from_str("Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi").unwrap()
            });

        let node_registry_program_id = std::env::var("NODE_REGISTRY_PROGRAM_ID")
            .ok()
            .map(|s| Pubkey::from_str(&s))
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid node registry program ID: {}", e))?
            .unwrap_or_else(|| {
                Pubkey::from_str("AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z").unwrap()
            });

        let payment_pool_program_id = std::env::var("PAYMENT_POOL_PROGRAM_ID")
            .ok()
            .map(|s| Pubkey::from_str(&s))
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid payment pool program ID: {}", e))?
            .unwrap_or_else(|| {
                Pubkey::from_str("4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX").unwrap()
            });

        Ok(Self {
            rpc_url,
            keypair_path,
            token_mint,
            node_registry_program_id,
            payment_pool_program_id,
        })
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

    /// Heartbeat interval (30 seconds)
    pub const HEARTBEAT_INTERVAL_SECONDS: u64 = 30;

    /// Heartbeat timeout (5 minutes)
    pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 300;

    /// Proof-of-storage challenge timeout (1 minute)
    pub const PROOF_CHALLENGE_TIMEOUT_SECONDS: i64 = 60;

    /// Slashing percentages
    pub const SLASH_DATA_LOSS_PERCENT: u8 = 10;
    pub const SLASH_EXTENDED_DOWNTIME_PERCENT: u8 = 5;
    pub const SLASH_CORRUPTED_DATA_PERCENT: u8 = 50;
    pub const SLASH_FAILED_PROOFS_PERCENT: u8 = 15;
}
