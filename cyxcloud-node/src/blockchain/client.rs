//! Storage Node Blockchain Client
//!
//! Main client for storage nodes to interact with Solana blockchain.

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
};
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use super::heartbeat::HeartbeatOps;
use super::registration::RegistrationOps;
use super::rewards::RewardsOps;
use super::types::{
    EpochClaimInfo, NodeBlockchainConfig, ProofChallenge, ProofOfStorage, StorageNodeInfo,
    StorageSpec,
};

/// Main blockchain client for storage nodes
pub struct StorageNodeBlockchainClient {
    /// Solana RPC client
    rpc_client: Arc<RpcClient>,

    /// Node owner keypair
    owner: Arc<Keypair>,

    /// Configuration
    config: NodeBlockchainConfig,

    /// Registration operations
    registration: RegistrationOps,

    /// Heartbeat operations
    heartbeat: HeartbeatOps,

    /// Rewards operations
    rewards: RewardsOps,
}

impl StorageNodeBlockchainClient {
    /// Create a new blockchain client
    pub fn new(config: NodeBlockchainConfig, owner: Keypair) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        ));

        let registration =
            RegistrationOps::new(rpc_client.clone(), config.node_registry_program_id);

        let heartbeat = HeartbeatOps::new(rpc_client.clone(), config.node_registry_program_id);

        let rewards = RewardsOps::new(
            rpc_client.clone(),
            config.payment_pool_program_id,
            config.node_registry_program_id,
            config.token_mint,
        );

        info!(
            rpc = %config.rpc_url,
            owner = %owner.pubkey(),
            "Storage node blockchain client initialized"
        );

        Ok(Self {
            rpc_client,
            owner: Arc::new(owner),
            config,
            registration,
            heartbeat,
            rewards,
        })
    }

    /// Create a new blockchain client from environment variables
    pub fn from_env() -> Result<Self> {
        let config = NodeBlockchainConfig::from_env()?;

        // Load keypair from file or generate ephemeral
        let owner = if let Some(ref path) = config.keypair_path {
            Self::load_keypair(path)?
        } else {
            warn!("No keypair path configured, using ephemeral keypair");
            Keypair::new()
        };

        Self::new(config, owner)
    }

    /// Load keypair from a file
    fn load_keypair(path: &str) -> Result<Keypair> {
        let path = Path::new(path);
        if !path.exists() {
            anyhow::bail!("Keypair file not found: {}", path.display());
        }

        let keypair_json = std::fs::read_to_string(path)?;
        let keypair_bytes: Vec<u8> = serde_json::from_str(&keypair_json)?;
        #[allow(deprecated)]
        let keypair = Keypair::from_bytes(&keypair_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse keypair: {}", e))?;

        info!(
            path = %path.display(),
            pubkey = %keypair.pubkey(),
            "Loaded node owner keypair"
        );

        Ok(keypair)
    }

    /// Get the node owner public key
    pub fn owner_pubkey(&self) -> Pubkey {
        self.owner.pubkey()
    }

    /// Get RPC client reference
    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    /// Get configuration reference
    pub fn config(&self) -> &NodeBlockchainConfig {
        &self.config
    }

    // =========================================================================
    // REGISTRATION OPERATIONS
    // =========================================================================

    /// Check if node is registered on-chain
    pub async fn is_registered(&self, node_id: &str) -> Result<bool> {
        self.registration
            .is_registered(&self.owner.pubkey(), node_id)
            .await
    }

    /// Get node information from on-chain
    pub async fn get_node_info(&self, node_id: &str) -> Result<Option<StorageNodeInfo>> {
        self.registration
            .get_node_info(&self.owner.pubkey(), node_id)
            .await
    }

    /// Register a new storage node
    pub async fn register_node(&self, node_id: &str, spec: &StorageSpec) -> Result<Signature> {
        self.registration
            .register_node(&self.owner, node_id, spec)
            .await
    }

    /// Add stake to node
    pub async fn add_stake(&self, node_id: &str, amount: u64) -> Result<Signature> {
        self.registration
            .add_stake(&self.owner, node_id, amount)
            .await
    }

    /// Request stake withdrawal (7-day lockup)
    pub async fn request_withdraw(&self, node_id: &str, amount: u64) -> Result<Signature> {
        self.registration
            .request_withdraw(&self.owner, node_id, amount)
            .await
    }

    /// Complete stake withdrawal (after lockup period)
    pub async fn complete_withdraw(&self, node_id: &str) -> Result<Signature> {
        self.registration
            .complete_withdraw(&self.owner, node_id)
            .await
    }

    // =========================================================================
    // HEARTBEAT OPERATIONS
    // =========================================================================

    /// Send heartbeat to update last_heartbeat timestamp
    pub async fn heartbeat(&self, node_id: &str) -> Result<Signature> {
        self.heartbeat.heartbeat(&self.owner, node_id).await
    }

    /// Submit proof-of-storage response
    pub async fn submit_proof(&self, node_id: &str, proof: &ProofOfStorage) -> Result<Signature> {
        self.heartbeat
            .submit_proof(&self.owner, node_id, proof)
            .await
    }

    /// Generate proof-of-storage from chunk data
    pub fn generate_proof(chunk_data: &[u8], challenge: &ProofChallenge) -> ProofOfStorage {
        HeartbeatOps::generate_proof(chunk_data, challenge)
    }

    /// Update node metrics on-chain
    pub async fn update_metrics(
        &self,
        node_id: &str,
        bytes_stored: u64,
        bytes_served: u64,
        uptime_seconds: u64,
        requests_served: u64,
    ) -> Result<Signature> {
        self.heartbeat
            .update_metrics(
                &self.owner,
                node_id,
                bytes_stored,
                bytes_served,
                uptime_seconds,
                requests_served,
            )
            .await
    }

    // =========================================================================
    // REWARDS OPERATIONS
    // =========================================================================

    /// Get current epoch number
    pub async fn get_current_epoch(&self) -> Result<u64> {
        self.rewards.get_current_epoch().await
    }

    /// Get epoch claim info for this node
    pub async fn get_epoch_claim(
        &self,
        node_id: &str,
        epoch: u64,
    ) -> Result<Option<EpochClaimInfo>> {
        self.rewards
            .get_epoch_claim(epoch, &self.owner.pubkey(), node_id)
            .await
    }

    /// Check if rewards are available for an epoch
    pub async fn has_claimable_rewards(&self, node_id: &str, epoch: u64) -> Result<bool> {
        self.rewards
            .has_claimable_rewards(epoch, &self.owner.pubkey(), node_id)
            .await
    }

    /// Claim rewards for an epoch
    pub async fn claim_rewards(&self, node_id: &str, epoch: u64) -> Result<Signature> {
        self.rewards
            .claim_rewards(&self.owner, node_id, epoch)
            .await
    }

    /// Get all pending rewards
    pub async fn get_pending_rewards(&self, node_id: &str) -> Result<Vec<EpochClaimInfo>> {
        self.rewards
            .get_pending_rewards(&self.owner.pubkey(), node_id)
            .await
    }

    /// Claim all pending rewards
    pub async fn claim_all_rewards(&self, node_id: &str) -> Result<Vec<Signature>> {
        self.rewards.claim_all_rewards(&self.owner, node_id).await
    }

    // =========================================================================
    // UTILITY METHODS
    // =========================================================================

    /// Get SOL balance of the node owner
    pub async fn get_sol_balance(&self) -> Result<u64> {
        Ok(self.rpc_client.get_balance(&self.owner.pubkey())?)
    }

    /// Get CYXWIZ token balance of the node owner
    pub async fn get_token_balance(&self) -> Result<u64> {
        let token_account =
            Self::get_associated_token_address(&self.owner.pubkey(), &self.config.token_mint);

        match self.rpc_client.get_token_account_balance(&token_account) {
            Ok(balance) => Ok(balance.amount.parse::<u64>().unwrap_or(0)),
            Err(_) => Ok(0), // Account doesn't exist
        }
    }

    /// Derive associated token account address
    fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
        use std::str::FromStr;
        const ATA_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
        const SPL_TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

        let ata_program = Pubkey::from_str(ATA_PROGRAM_ID).unwrap();
        let token_program = Pubkey::from_str(SPL_TOKEN_PROGRAM_ID).unwrap();

        let (address, _) = Pubkey::find_program_address(
            &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
            &ata_program,
        );
        address
    }

    /// Check if the node owner has enough SOL for transactions
    pub async fn has_sufficient_sol(&self, min_lamports: u64) -> Result<bool> {
        let balance = self.get_sol_balance().await?;
        Ok(balance >= min_lamports)
    }

    /// Derive the storage node PDA for this owner and node_id
    pub fn derive_node_pda(&self, node_id: &str) -> (Pubkey, u8) {
        self.registration
            .derive_node_pda(&self.owner.pubkey(), node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = NodeBlockchainConfig::default();
        assert_eq!(config.rpc_url, "https://api.devnet.solana.com");
    }

    #[test]
    fn test_program_ids_configured() {
        let config = NodeBlockchainConfig::default();

        // Verify all program IDs are set
        assert_ne!(config.token_mint, Pubkey::default());
        assert_ne!(config.node_registry_program_id, Pubkey::default());
        assert_ne!(config.payment_pool_program_id, Pubkey::default());

        // Verify specific program IDs (deployed 2025-12-22)
        assert_eq!(
            config.token_mint.to_string(),
            "Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi"
        );
        assert_eq!(
            config.node_registry_program_id.to_string(),
            "AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z"
        );
        assert_eq!(
            config.payment_pool_program_id.to_string(),
            "4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX"
        );
    }

    #[test]
    fn test_storage_spec_creation() {
        use super::super::types::DiskType;

        let spec = StorageSpec::new(
            100 * 1024 * 1024 * 1024, // 100 GB
            DiskType::NVMe,
            "us-east-1",
            1000,
        );

        assert_eq!(spec.total_capacity_bytes, 100 * 1024 * 1024 * 1024);
        assert_eq!(spec.disk_type, DiskType::NVMe);
        assert_eq!(spec.region, "us-east-1");
        assert_eq!(spec.bandwidth_mbps, 1000);
    }
}
