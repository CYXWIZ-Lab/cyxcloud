//! Payment Pool Operations
//!
//! Handles StoragePaymentPool program interactions for epoch management and rewards.

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use sha2::{Digest, Sha256};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;

/// System program ID
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";

/// SPL Token program ID
const SPL_TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Get system program ID
fn system_program_id() -> Pubkey {
    Pubkey::from_str(SYSTEM_PROGRAM_ID).unwrap()
}

/// Get SPL Token program ID
fn spl_token_program_id() -> Pubkey {
    Pubkey::from_str(SPL_TOKEN_PROGRAM_ID).unwrap()
}
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::types::{constants, EpochRewardsInfo, NodeEpochClaimInfo, PaymentPoolInfo};

/// Get Anchor discriminator for an instruction
fn get_discriminator(instruction_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{}", instruction_name));
    let result = hasher.finalize();
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&result[..8]);
    discriminator
}

/// Payment pool operations for the Gateway
pub struct PaymentPoolOps {
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
    token_mint: Pubkey,
}

impl PaymentPoolOps {
    /// Create new payment pool operations handler
    pub fn new(rpc_client: Arc<RpcClient>, program_id: Pubkey, token_mint: Pubkey) -> Self {
        Self {
            rpc_client,
            program_id,
            token_mint,
        }
    }

    /// Get pool PDA
    pub fn get_pool_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"pool"], &self.program_id)
    }

    /// Get epoch rewards PDA
    pub fn get_epoch_rewards_pda(&self, epoch: u64) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"epoch_rewards", &epoch.to_le_bytes()],
            &self.program_id,
        )
    }

    /// Get node claim PDA
    pub fn get_node_claim_pda(
        &self,
        epoch: u64,
        node_owner: &Pubkey,
        node_id: &str,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"node_claim",
                &epoch.to_le_bytes(),
                node_owner.as_ref(),
                node_id.as_bytes(),
            ],
            &self.program_id,
        )
    }

    /// Get payment pool info
    pub async fn get_pool(&self) -> Result<Option<PaymentPoolInfo>> {
        let (pool_pda, _) = self.get_pool_pda();

        match self.rpc_client.get_account_data(&pool_pda) {
            Ok(data) => {
                if data.len() < 8 {
                    return Ok(None);
                }
                let pool = self.parse_pool_data(&data[8..])?;
                Ok(Some(pool))
            }
            Err(_) => Ok(None),
        }
    }

    /// Parse pool data from raw bytes
    fn parse_pool_data(&self, data: &[u8]) -> Result<PaymentPoolInfo> {
        if data.len() < 120 {
            anyhow::bail!("Pool data too short");
        }

        let authority = Pubkey::try_from(&data[0..32])?;
        let token_mint = Pubkey::try_from(&data[32..64])?;
        let current_epoch = u64::from_le_bytes(data[64..72].try_into()?);
        let total_deposits = u64::from_le_bytes(data[72..80].try_into()?);
        let node_percent = data[80];
        let platform_percent = data[81];
        let community_percent = data[82];
        let epoch_duration = i64::from_le_bytes(data[83..91].try_into()?);
        let last_epoch_start = i64::from_le_bytes(data[91..99].try_into()?);

        Ok(PaymentPoolInfo {
            authority,
            token_mint,
            current_epoch,
            total_deposits,
            node_percent,
            platform_percent,
            community_percent,
            epoch_duration_seconds: epoch_duration,
            last_epoch_start,
        })
    }

    /// Get epoch rewards info
    pub async fn get_epoch_rewards(&self, epoch: u64) -> Result<Option<EpochRewardsInfo>> {
        let (epoch_pda, _) = self.get_epoch_rewards_pda(epoch);

        match self.rpc_client.get_account_data(&epoch_pda) {
            Ok(data) => {
                if data.len() < 8 {
                    return Ok(None);
                }
                let rewards = self.parse_epoch_rewards_data(&data[8..], epoch)?;
                Ok(Some(rewards))
            }
            Err(_) => Ok(None),
        }
    }

    /// Parse epoch rewards data from raw bytes
    fn parse_epoch_rewards_data(&self, data: &[u8], epoch: u64) -> Result<EpochRewardsInfo> {
        if data.len() < 72 {
            anyhow::bail!("Epoch rewards data too short");
        }

        let total_pool_amount = u64::from_le_bytes(data[8..16].try_into()?);
        let nodes_share = u64::from_le_bytes(data[16..24].try_into()?);
        let platform_share = u64::from_le_bytes(data[24..32].try_into()?);
        let community_share = u64::from_le_bytes(data[32..40].try_into()?);
        let allocated_to_nodes = u64::from_le_bytes(data[40..48].try_into()?);
        let finalized = data[48] != 0;
        let platform_claimed = data[49] != 0;
        let community_claimed = data[50] != 0;
        let epoch_start = i64::from_le_bytes(data[51..59].try_into()?);
        let epoch_end = i64::from_le_bytes(data[59..67].try_into()?);

        Ok(EpochRewardsInfo {
            epoch,
            total_pool_amount,
            nodes_share,
            platform_share,
            community_share,
            allocated_to_nodes,
            finalized,
            platform_claimed,
            community_claimed,
            epoch_start,
            epoch_end,
        })
    }

    /// Start a new epoch (Gateway authority only)
    pub async fn start_new_epoch(&self, authority: &Keypair) -> Result<String> {
        let (pool_pda, _) = self.get_pool_pda();

        // Get current epoch
        let pool = self.get_pool().await?.ok_or_else(|| anyhow::anyhow!("Pool not found"))?;
        let current_epoch = pool.current_epoch;

        let (epoch_rewards_pda, _) = self.get_epoch_rewards_pda(current_epoch);

        // Build instruction data
        let discriminator = get_discriminator("start_new_epoch");
        let ix_data = discriminator.to_vec();

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new(epoch_rewards_pda, false),
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new_readonly(system_program_id(), false),
            ],
            data: ix_data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&authority.pubkey()),
            &[authority],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!(
            epoch = current_epoch,
            signature = %signature,
            "New epoch started"
        );

        Ok(signature.to_string())
    }

    /// Allocate node reward for an epoch (Gateway authority only)
    pub async fn allocate_node_reward(
        &self,
        authority: &Keypair,
        epoch: u64,
        node_owner: &Pubkey,
        node_id: &str,
        reward_amount: u64,
    ) -> Result<String> {
        let (pool_pda, _) = self.get_pool_pda();
        let (epoch_rewards_pda, _) = self.get_epoch_rewards_pda(epoch);
        let (node_claim_pda, _) = self.get_node_claim_pda(epoch, node_owner, node_id);

        // Build instruction data
        let discriminator = get_discriminator("allocate_node_reward");
        let mut ix_data = Vec::with_capacity(56 + node_id.len());
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&epoch.to_le_bytes());
        ix_data.extend_from_slice(node_owner.as_ref());
        ix_data.extend_from_slice(&(node_id.len() as u32).to_le_bytes());
        ix_data.extend_from_slice(node_id.as_bytes());
        ix_data.extend_from_slice(&reward_amount.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new(epoch_rewards_pda, false),
                AccountMeta::new(node_claim_pda, false),
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new_readonly(system_program_id(), false),
            ],
            data: ix_data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&authority.pubkey()),
            &[authority],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!(
            epoch = epoch,
            node_owner = %node_owner,
            node_id = node_id,
            reward = reward_amount,
            signature = %signature,
            "Node reward allocated"
        );

        Ok(signature.to_string())
    }

    /// Finalize epoch (Gateway authority only)
    pub async fn finalize_epoch(&self, authority: &Keypair, epoch: u64) -> Result<String> {
        let (pool_pda, _) = self.get_pool_pda();
        let (epoch_rewards_pda, _) = self.get_epoch_rewards_pda(epoch);

        // Build instruction data
        let discriminator = get_discriminator("finalize_epoch");
        let mut ix_data = Vec::with_capacity(16);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&epoch.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(pool_pda, false),
                AccountMeta::new(epoch_rewards_pda, false),
                AccountMeta::new_readonly(authority.pubkey(), true),
            ],
            data: ix_data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&authority.pubkey()),
            &[authority],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!(
            epoch = epoch,
            signature = %signature,
            "Epoch finalized"
        );

        Ok(signature.to_string())
    }

    /// Claim platform share (Gateway authority only)
    pub async fn claim_platform_share(
        &self,
        authority: &Keypair,
        epoch: u64,
        pool_token_account: &Pubkey,
        platform_token_account: &Pubkey,
    ) -> Result<String> {
        let (pool_pda, _) = self.get_pool_pda();
        let (epoch_rewards_pda, _) = self.get_epoch_rewards_pda(epoch);

        // Build instruction data
        let discriminator = get_discriminator("claim_platform_share");
        let mut ix_data = Vec::with_capacity(16);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&epoch.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(pool_pda, false),
                AccountMeta::new(epoch_rewards_pda, false),
                AccountMeta::new(*pool_token_account, false),
                AccountMeta::new(*platform_token_account, false),
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new_readonly(spl_token_program_id(), false),
            ],
            data: ix_data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&authority.pubkey()),
            &[authority],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!(
            epoch = epoch,
            signature = %signature,
            "Platform share claimed"
        );

        Ok(signature.to_string())
    }

    /// Claim community share (Gateway authority only)
    pub async fn claim_community_share(
        &self,
        authority: &Keypair,
        epoch: u64,
        pool_token_account: &Pubkey,
        community_token_account: &Pubkey,
    ) -> Result<String> {
        let (pool_pda, _) = self.get_pool_pda();
        let (epoch_rewards_pda, _) = self.get_epoch_rewards_pda(epoch);

        // Build instruction data
        let discriminator = get_discriminator("claim_community_share");
        let mut ix_data = Vec::with_capacity(16);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&epoch.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(pool_pda, false),
                AccountMeta::new(epoch_rewards_pda, false),
                AccountMeta::new(*pool_token_account, false),
                AccountMeta::new(*community_token_account, false),
                AccountMeta::new_readonly(authority.pubkey(), true),
                AccountMeta::new_readonly(spl_token_program_id(), false),
            ],
            data: ix_data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&authority.pubkey()),
            &[authority],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;
        info!(
            epoch = epoch,
            signature = %signature,
            "Community share claimed"
        );

        Ok(signature.to_string())
    }

    /// Check if current epoch should end
    pub async fn should_end_epoch(&self) -> Result<bool> {
        match self.get_pool().await? {
            Some(pool) => {
                let now = chrono::Utc::now().timestamp();
                let epoch_end = pool.last_epoch_start + pool.epoch_duration_seconds;
                Ok(now >= epoch_end)
            }
            None => Ok(false),
        }
    }

    /// Get current epoch number
    pub async fn current_epoch(&self) -> Result<u64> {
        match self.get_pool().await? {
            Some(pool) => Ok(pool.current_epoch),
            None => Ok(0),
        }
    }

    /// Calculate node's share of epoch rewards
    pub fn calculate_node_share(
        total_nodes_share: u64,
        node_weight: u64,
        total_weight: u64,
    ) -> u64 {
        if total_weight == 0 {
            return 0;
        }
        (total_nodes_share as u128 * node_weight as u128 / total_weight as u128) as u64
    }
}

/// Node weight calculation for reward distribution
pub struct NodeWeight {
    pub owner: Pubkey,
    pub node_id: String,
    pub weight: u64,
}

impl PaymentPoolOps {
    /// Calculate weights for all active nodes
    /// Weight is based on: storage * uptime_factor * reputation_factor
    pub fn calculate_node_weights(
        nodes: &[(Pubkey, String, u64, u64, u16)], // (owner, id, storage_bytes, uptime_seconds, reputation)
    ) -> Vec<NodeWeight> {
        nodes
            .iter()
            .map(|(owner, id, storage, uptime, reputation)| {
                // Uptime factor: based on seconds online in epoch
                let uptime_factor = (*uptime as f64 / constants::EPOCH_DURATION_SECONDS as f64)
                    .min(1.0);

                // Reputation factor: 0.5 to 1.5 based on score
                let reputation_factor = 0.5 + (*reputation as f64 / 10000.0);

                // Weight = storage * uptime * reputation
                let weight = (*storage as f64 * uptime_factor * reputation_factor) as u64;

                NodeWeight {
                    owner: *owner,
                    node_id: id.clone(),
                    weight,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_pda() {
        let program_id = Pubkey::new_unique();
        let rpc = Arc::new(RpcClient::new("http://localhost:8899".to_string()));
        let ops = PaymentPoolOps::new(rpc, program_id, Pubkey::new_unique());

        let (pda, bump) = ops.get_pool_pda();
        assert!(bump <= 255);
        assert_ne!(pda, program_id);
    }

    #[test]
    fn test_calculate_node_share() {
        // Equal split among 4 nodes
        let share = PaymentPoolOps::calculate_node_share(1000, 250, 1000);
        assert_eq!(share, 250);

        // Unequal split
        let share = PaymentPoolOps::calculate_node_share(1000, 600, 1000);
        assert_eq!(share, 600);

        // Zero total weight
        let share = PaymentPoolOps::calculate_node_share(1000, 0, 0);
        assert_eq!(share, 0);
    }

    #[test]
    fn test_calculate_node_weights() {
        let nodes = vec![
            (
                Pubkey::new_unique(),
                "node1".to_string(),
                1_000_000_000_000, // 1 TB
                constants::EPOCH_DURATION_SECONDS as u64,
                10000, // Perfect reputation
            ),
            (
                Pubkey::new_unique(),
                "node2".to_string(),
                500_000_000_000, // 500 GB
                constants::EPOCH_DURATION_SECONDS as u64 / 2,
                5000, // Average reputation
            ),
        ];

        let weights = PaymentPoolOps::calculate_node_weights(&nodes);
        assert_eq!(weights.len(), 2);

        // First node should have higher weight (more storage, full uptime, perfect reputation)
        assert!(weights[0].weight > weights[1].weight);
    }
}
