//! Reward Claiming Operations
//!
//! Handles claiming epoch rewards from the payment pool.

use anyhow::Result;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use std::sync::Arc;
use tracing::{info, warn};

use super::types::EpochClaimInfo;

/// SPL Token program ID
const SPL_TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Associated Token Account program ID
const ATA_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// Get the SPL Token program ID
fn spl_token_program_id() -> Pubkey {
    use std::str::FromStr;
    Pubkey::from_str(SPL_TOKEN_PROGRAM_ID).unwrap()
}

/// Derive associated token account address
fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    use std::str::FromStr;
    let ata_program = Pubkey::from_str(ATA_PROGRAM_ID).unwrap();
    let token_program = spl_token_program_id();

    let (address, _) = Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program,
    );
    address
}

/// Reward claiming operations for storage nodes
pub struct RewardsOps {
    rpc_client: Arc<RpcClient>,
    payment_pool_program_id: Pubkey,
    node_registry_program_id: Pubkey,
    token_mint: Pubkey,
}

impl RewardsOps {
    /// Create new rewards operations handler
    pub fn new(
        rpc_client: Arc<RpcClient>,
        payment_pool_program_id: Pubkey,
        node_registry_program_id: Pubkey,
        token_mint: Pubkey,
    ) -> Self {
        Self {
            rpc_client,
            payment_pool_program_id,
            node_registry_program_id,
            token_mint,
        }
    }

    /// Derive the payment pool PDA
    fn derive_pool_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"pool"], &self.payment_pool_program_id)
    }

    /// Derive the epoch rewards PDA
    fn derive_epoch_pda(&self, epoch: u64) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"epoch", &epoch.to_le_bytes()],
            &self.payment_pool_program_id,
        )
    }

    /// Derive the node epoch claim PDA
    fn derive_claim_pda(&self, epoch: u64, owner: &Pubkey, node_id: &str) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"node_claim",
                &epoch.to_le_bytes(),
                owner.as_ref(),
                node_id.as_bytes(),
            ],
            &self.payment_pool_program_id,
        )
    }

    /// Derive the storage node PDA
    fn derive_node_pda(&self, owner: &Pubkey, node_id: &str) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"storage_node", owner.as_ref(), node_id.as_bytes()],
            &self.node_registry_program_id,
        )
    }

    /// Get Anchor discriminator for an instruction
    fn get_discriminator(instruction_name: &str) -> [u8; 8] {
        let mut hasher = Sha256::new();
        hasher.update(format!("global:{}", instruction_name));
        let result = hasher.finalize();
        let mut discriminator = [0u8; 8];
        discriminator.copy_from_slice(&result[..8]);
        discriminator
    }

    /// Get current epoch number from pool
    pub async fn get_current_epoch(&self) -> Result<u64> {
        let (pool_pda, _) = self.derive_pool_pda();

        let account = self.rpc_client.get_account(&pool_pda)?;
        let data = &account.data;

        // Parse current_epoch from pool account
        // Format: discriminator(8) + authority(32) + token_mint(32) + current_epoch(8) + ...
        if data.len() < 80 {
            anyhow::bail!("Invalid pool account data");
        }

        let current_epoch = u64::from_le_bytes(data[72..80].try_into()?);
        Ok(current_epoch)
    }

    /// Get epoch claim info for a node
    pub async fn get_epoch_claim(
        &self,
        epoch: u64,
        owner: &Pubkey,
        node_id: &str,
    ) -> Result<Option<EpochClaimInfo>> {
        let (claim_pda, _) = self.derive_claim_pda(epoch, owner, node_id);

        let account = match self.rpc_client.get_account(&claim_pda) {
            Ok(acc) => acc,
            Err(_) => return Ok(None),
        };

        let data = &account.data;
        if data.len() < 8 {
            return Ok(None);
        }

        // Parse claim account
        // Format: discriminator(8) + epoch(8) + node_owner(32) + node_id(4+len) + reward_amount(8) + claimed(1)
        let mut offset = 8; // Skip discriminator

        let claim_epoch = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        // Skip node_owner and node_id
        offset += 32; // node_owner
        let node_id_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4 + node_id_len;

        let reward_amount = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let claimed = data[offset] != 0;

        Ok(Some(EpochClaimInfo {
            epoch: claim_epoch,
            reward_amount,
            claimed,
        }))
    }

    /// Check if rewards are available for an epoch
    pub async fn has_claimable_rewards(
        &self,
        epoch: u64,
        owner: &Pubkey,
        node_id: &str,
    ) -> Result<bool> {
        match self.get_epoch_claim(epoch, owner, node_id).await? {
            Some(claim) => Ok(!claim.claimed && claim.reward_amount > 0),
            None => Ok(false),
        }
    }

    /// Claim rewards for an epoch
    pub async fn claim_rewards(
        &self,
        owner: &Keypair,
        node_id: &str,
        epoch: u64,
    ) -> Result<Signature> {
        let (pool_pda, _) = self.derive_pool_pda();
        let (epoch_pda, _) = self.derive_epoch_pda(epoch);
        let (claim_pda, _) = self.derive_claim_pda(epoch, &owner.pubkey(), node_id);
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        // Get pool token account
        let pool_token_account = get_associated_token_address(&pool_pda, &self.token_mint);

        // Get owner token account
        let owner_token_account = get_associated_token_address(&owner.pubkey(), &self.token_mint);

        info!(
            node_id = %node_id,
            epoch = epoch,
            "Claiming epoch rewards"
        );

        let discriminator = Self::get_discriminator("claim_node_rewards");

        // Build instruction data: discriminator + epoch
        let mut data = Vec::with_capacity(8 + 8);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&epoch.to_le_bytes());

        let instruction = Instruction {
            program_id: self.payment_pool_program_id,
            accounts: vec![
                AccountMeta::new(pool_pda, false),
                AccountMeta::new(epoch_pda, false),
                AccountMeta::new(claim_pda, false),
                AccountMeta::new_readonly(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(pool_token_account, false),
                AccountMeta::new(owner_token_account, false),
                AccountMeta::new_readonly(spl_token_program_id(), false),
            ],
            data,
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&owner.pubkey()),
            &[owner],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;

        info!(
            signature = %signature,
            epoch = epoch,
            "Rewards claimed successfully"
        );

        Ok(signature)
    }

    /// Get pending rewards across multiple epochs
    pub async fn get_pending_rewards(
        &self,
        owner: &Pubkey,
        node_id: &str,
    ) -> Result<Vec<EpochClaimInfo>> {
        let current_epoch = self.get_current_epoch().await?;
        let mut pending = Vec::new();

        // Check last 10 epochs for unclaimed rewards
        let start_epoch = current_epoch.saturating_sub(10);
        for epoch in start_epoch..current_epoch {
            if let Some(claim) = self.get_epoch_claim(epoch, owner, node_id).await? {
                if !claim.claimed && claim.reward_amount > 0 {
                    pending.push(claim);
                }
            }
        }

        Ok(pending)
    }

    /// Claim all pending rewards
    pub async fn claim_all_rewards(
        &self,
        owner: &Keypair,
        node_id: &str,
    ) -> Result<Vec<Signature>> {
        let pending = self.get_pending_rewards(&owner.pubkey(), node_id).await?;
        let mut signatures = Vec::new();

        for claim in pending {
            match self.claim_rewards(owner, node_id, claim.epoch).await {
                Ok(sig) => {
                    info!(
                        epoch = claim.epoch,
                        amount_cyxwiz = claim.reward_amount / 1_000_000_000,
                        signature = %sig,
                        "Claimed epoch reward"
                    );
                    signatures.push(sig);
                }
                Err(e) => {
                    warn!(
                        epoch = claim.epoch,
                        error = %e,
                        "Failed to claim epoch reward"
                    );
                }
            }
        }

        Ok(signatures)
    }
}
