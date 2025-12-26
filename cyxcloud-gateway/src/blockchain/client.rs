//! CyxCloud Blockchain Client
//!
//! Main client for interacting with CyxCloud Solana programs:
//! - StorageSubscription: User storage plans and usage
//! - StorageNodeRegistry: Node registration and proof-of-storage
//! - StoragePaymentPool: Payment epochs and reward distribution

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::node_registry::NodeRegistryOps;
use super::payment_pool::PaymentPoolOps;
use super::subscription::SubscriptionOps;
use super::types::{BlockchainConfig, StorageMetrics, SubscriptionInfo};

/// Main blockchain client for the CyxCloud Gateway
pub struct CyxCloudBlockchainClient {
    /// Solana RPC client
    rpc_client: Arc<RpcClient>,

    /// Gateway authority keypair
    authority: Arc<Keypair>,

    /// Configuration
    config: BlockchainConfig,

    /// Subscription operations
    subscription: SubscriptionOps,

    /// Node registry operations
    node_registry: NodeRegistryOps,

    /// Payment pool operations
    payment_pool: PaymentPoolOps,
}

impl CyxCloudBlockchainClient {
    /// Create a new blockchain client
    pub fn new(config: BlockchainConfig, authority: Keypair) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            CommitmentConfig::confirmed(),
        ));

        let subscription = SubscriptionOps::new(
            rpc_client.clone(),
            config.subscription_program_id,
            config.token_mint,
        );

        let node_registry = NodeRegistryOps::new(
            rpc_client.clone(),
            config.node_registry_program_id,
        );

        let payment_pool = PaymentPoolOps::new(
            rpc_client.clone(),
            config.payment_pool_program_id,
            config.token_mint,
        );

        info!(
            rpc = %config.rpc_url,
            authority = %authority.pubkey(),
            "Blockchain client initialized"
        );

        Ok(Self {
            rpc_client,
            authority: Arc::new(authority),
            config,
            subscription,
            node_registry,
            payment_pool,
        })
    }

    /// Create a new blockchain client from environment variables
    pub fn from_env() -> Result<Self> {
        let config = BlockchainConfig::from_env()?;

        // Load keypair from file or generate ephemeral
        let authority = if let Some(ref path) = config.keypair_path {
            Self::load_keypair(path)?
        } else {
            warn!("No keypair path configured, using ephemeral keypair");
            Keypair::new()
        };

        Self::new(config, authority)
    }

    /// Load keypair from a file
    fn load_keypair(path: &str) -> Result<Keypair> {
        let path = Path::new(path);
        if !path.exists() {
            anyhow::bail!("Keypair file not found: {}", path.display());
        }

        let keypair_json = std::fs::read_to_string(path)?;
        let keypair_bytes: Vec<u8> = serde_json::from_str(&keypair_json)
            .map_err(|e| anyhow::anyhow!("Failed to parse keypair JSON: {}", e))?;
        #[allow(deprecated)]
        let keypair = Keypair::from_bytes(&keypair_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse keypair: {}", e))?;

        info!(
            path = %path.display(),
            pubkey = %keypair.pubkey(),
            "Loaded gateway authority keypair"
        );

        Ok(keypair)
    }

    /// Get the gateway authority public key
    pub fn authority_pubkey(&self) -> Pubkey {
        self.authority.pubkey()
    }

    /// Get RPC client reference
    pub fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    /// Get configuration reference
    pub fn config(&self) -> &BlockchainConfig {
        &self.config
    }

    // =========================================================================
    // SUBSCRIPTION OPERATIONS
    // =========================================================================

    /// Get subscription info for a user
    pub async fn get_subscription(&self, user: &Pubkey) -> Result<Option<SubscriptionInfo>> {
        self.subscription.get_subscription(user).await
    }

    /// Check if a user has an active subscription
    pub async fn is_subscription_active(&self, user: &Pubkey) -> Result<bool> {
        self.subscription.is_active(user).await
    }

    /// Get remaining storage quota for a user
    pub async fn get_remaining_quota(&self, user: &Pubkey) -> Result<u64> {
        self.subscription.get_remaining_quota(user).await
    }

    /// Check if user has enough quota for additional storage
    pub async fn has_quota(&self, user: &Pubkey, bytes: u64) -> Result<bool> {
        self.subscription.has_quota(user, bytes).await
    }

    /// Update storage and bandwidth usage for a user
    pub async fn update_usage(
        &self,
        user: &Pubkey,
        storage_used: u64,
        bandwidth_used: u64,
    ) -> Result<String> {
        self.subscription
            .update_usage(&self.authority, user, storage_used, bandwidth_used)
            .await
    }

    // =========================================================================
    // NODE REGISTRY OPERATIONS
    // =========================================================================

    /// Get storage node info
    pub async fn get_node(
        &self,
        owner: &Pubkey,
        node_id: &str,
    ) -> Result<Option<super::types::StorageNodeInfo>> {
        self.node_registry.get_node(owner, node_id).await
    }

    /// Check if a node is active
    pub async fn is_node_active(&self, owner: &Pubkey, node_id: &str) -> Result<bool> {
        self.node_registry.is_node_active(owner, node_id).await
    }

    /// Update node metrics
    pub async fn update_node_metrics(
        &self,
        owner: &Pubkey,
        node_id: &str,
        metrics: &StorageMetrics,
    ) -> Result<String> {
        self.node_registry
            .update_metrics(&self.authority, owner, node_id, metrics)
            .await
    }

    /// Slash a node for misbehavior
    pub async fn slash_node(
        &self,
        owner: &Pubkey,
        node_id: &str,
        amount: u64,
        reason: &str,
    ) -> Result<String> {
        self.node_registry
            .slash_node(&self.authority, owner, node_id, amount, reason)
            .await
    }

    // =========================================================================
    // PAYMENT POOL OPERATIONS
    // =========================================================================

    /// Get current epoch number
    pub async fn current_epoch(&self) -> Result<u64> {
        self.payment_pool.current_epoch().await
    }

    /// Check if current epoch should end
    pub async fn should_end_epoch(&self) -> Result<bool> {
        self.payment_pool.should_end_epoch().await
    }

    /// Start a new payment epoch
    pub async fn start_new_epoch(&self) -> Result<String> {
        self.payment_pool.start_new_epoch(&self.authority).await
    }

    /// Allocate reward to a node for an epoch
    pub async fn allocate_node_reward(
        &self,
        epoch: u64,
        node_owner: &Pubkey,
        node_id: &str,
        reward_amount: u64,
    ) -> Result<String> {
        self.payment_pool
            .allocate_node_reward(&self.authority, epoch, node_owner, node_id, reward_amount)
            .await
    }

    /// Finalize an epoch
    pub async fn finalize_epoch(&self, epoch: u64) -> Result<String> {
        self.payment_pool.finalize_epoch(&self.authority, epoch).await
    }

    /// Get epoch rewards info
    pub async fn get_epoch_rewards(&self, epoch: u64) -> Result<Option<super::types::EpochRewardsInfo>> {
        self.payment_pool.get_epoch_rewards(epoch).await
    }

    /// Claim platform share for an epoch
    pub async fn claim_platform_share(
        &self,
        epoch: u64,
        pool_token_account: &Pubkey,
    ) -> Result<String> {
        self.payment_pool
            .claim_platform_share(
                &self.authority,
                epoch,
                pool_token_account,
                &self.config.platform_treasury,
            )
            .await
    }

    /// Claim community share for an epoch
    pub async fn claim_community_share(
        &self,
        epoch: u64,
        pool_token_account: &Pubkey,
    ) -> Result<String> {
        self.payment_pool
            .claim_community_share(
                &self.authority,
                epoch,
                pool_token_account,
                &self.config.community_fund,
            )
            .await
    }

    // =========================================================================
    // UTILITY METHODS
    // =========================================================================

    /// Get SOL balance of an account
    pub async fn get_sol_balance(&self, pubkey: &Pubkey) -> Result<u64> {
        Ok(self.rpc_client.get_balance(pubkey)?)
    }

    /// Get token balance of an account
    pub async fn get_token_balance(&self, token_account: &Pubkey) -> Result<u64> {
        let account = self.rpc_client.get_token_account_balance(token_account)?;
        Ok(account.amount.parse::<u64>().unwrap_or(0))
    }

    /// Check if the gateway has enough SOL for transactions
    pub async fn has_sufficient_sol(&self, min_lamports: u64) -> Result<bool> {
        let balance = self.get_sol_balance(&self.authority.pubkey()).await?;
        Ok(balance >= min_lamports)
    }
}

impl BlockchainConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());

        let keypair_path = std::env::var("GATEWAY_KEYPAIR_PATH").ok();

        let token_mint = std::env::var("CYXWIZ_TOKEN_MINT")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid token mint: {}", e))?
            .unwrap_or_default();

        let subscription_program_id = std::env::var("SUBSCRIPTION_PROGRAM_ID")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid subscription program ID: {}", e))?
            .unwrap_or_default();

        let node_registry_program_id = std::env::var("NODE_REGISTRY_PROGRAM_ID")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid node registry program ID: {}", e))?
            .unwrap_or_default();

        let payment_pool_program_id = std::env::var("PAYMENT_POOL_PROGRAM_ID")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid payment pool program ID: {}", e))?
            .unwrap_or_default();

        let platform_treasury = std::env::var("PLATFORM_TREASURY")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid platform treasury: {}", e))?
            .unwrap_or_default();

        let community_fund = std::env::var("COMMUNITY_FUND")
            .ok()
            .map(|s| s.parse::<Pubkey>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Invalid community fund: {}", e))?
            .unwrap_or_default();

        Ok(Self {
            rpc_url,
            keypair_path,
            token_mint,
            subscription_program_id,
            node_registry_program_id,
            payment_pool_program_id,
            platform_treasury,
            community_fund,
        })
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = BlockchainConfig::default();
        assert_eq!(config.rpc_url, "https://api.devnet.solana.com");
    }

    #[test]
    fn test_program_ids_configured() {
        let config = BlockchainConfig::default();

        // Verify all program IDs are set (not default/zero)
        assert_ne!(config.token_mint, Pubkey::default());
        assert_ne!(config.subscription_program_id, Pubkey::default());
        assert_ne!(config.node_registry_program_id, Pubkey::default());
        assert_ne!(config.payment_pool_program_id, Pubkey::default());
        assert_ne!(config.platform_treasury, Pubkey::default());

        // Verify specific program IDs (deployed 2025-12-22)
        assert_eq!(
            config.token_mint.to_string(),
            "Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi"
        );
        assert_eq!(
            config.subscription_program_id.to_string(),
            "HZhWDJVkkUuHrgqkNb9bYxiCfqtnv8cnus9UQt843Fro"
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
}
