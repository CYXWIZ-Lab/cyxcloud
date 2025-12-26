//! Node Registration Operations
//!
//! Handles storage node registration, staking, and status updates.

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
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, info};

use super::types::{DiskType, StorageNodeInfo, StorageNodeStatus, StorageSpec};

/// System program ID
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";

/// SPL Token program ID
const SPL_TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// Associated Token Account program ID
const ATA_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// Get the system program ID
fn system_program_id() -> Pubkey {
    Pubkey::from_str(SYSTEM_PROGRAM_ID).unwrap()
}

/// Get the SPL Token program ID
fn spl_token_program_id() -> Pubkey {
    Pubkey::from_str(SPL_TOKEN_PROGRAM_ID).unwrap()
}

/// Derive associated token account address
fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    let ata_program = Pubkey::from_str(ATA_PROGRAM_ID).unwrap();
    let token_program = spl_token_program_id();

    let (address, _) = Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program,
    );
    address
}

/// Registration operations for storage nodes
pub struct RegistrationOps {
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
}

impl RegistrationOps {
    /// Create new registration operations handler
    pub fn new(rpc_client: Arc<RpcClient>, program_id: Pubkey) -> Self {
        Self {
            rpc_client,
            program_id,
        }
    }

    /// Derive the registry PDA
    pub fn derive_registry_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"registry"], &self.program_id)
    }

    /// Derive the storage node PDA
    pub fn derive_node_pda(&self, owner: &Pubkey, node_id: &str) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"storage_node", owner.as_ref(), node_id.as_bytes()],
            &self.program_id,
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

    /// Check if a node is already registered
    pub async fn is_registered(&self, owner: &Pubkey, node_id: &str) -> Result<bool> {
        let (node_pda, _) = self.derive_node_pda(owner, node_id);
        let account = self.rpc_client.get_account(&node_pda);
        Ok(account.is_ok())
    }

    /// Get node information from on-chain
    pub async fn get_node_info(&self, owner: &Pubkey, node_id: &str) -> Result<Option<StorageNodeInfo>> {
        let (node_pda, _) = self.derive_node_pda(owner, node_id);

        let account = match self.rpc_client.get_account(&node_pda) {
            Ok(acc) => acc,
            Err(_) => return Ok(None),
        };

        // Parse account data
        // Format: discriminator(8) + node_id(4+len) + owner(32) + stake(8) + spec + metrics + reputation + status + timestamps
        let data = &account.data;
        if data.len() < 8 {
            return Ok(None);
        }

        let mut offset = 8; // Skip discriminator

        // Parse node_id (Anchor string: 4-byte length + UTF-8)
        let node_id_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        let parsed_node_id = String::from_utf8_lossy(&data[offset..offset + node_id_len]).to_string();
        offset += node_id_len;

        // owner (32 bytes)
        let parsed_owner = Pubkey::try_from(&data[offset..offset + 32])?;
        offset += 32;

        // stake_amount (u64)
        let stake_amount = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        // Storage spec
        let total_capacity_bytes = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let allocated_capacity_bytes = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let disk_type = DiskType::from_byte(data[offset]);
        offset += 1;

        // region (Anchor string)
        let region_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;
        let region = String::from_utf8_lossy(&data[offset..offset + region_len]).to_string();
        offset += region_len;

        let bandwidth_mbps = u32::from_le_bytes(data[offset..offset + 4].try_into()?);
        offset += 4;

        // Metrics
        offset += 8 * 4; // Skip bytes_stored, bytes_served, uptime_seconds, requests_served

        // Reputation
        let total_proofs = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let failed_proofs = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        offset += 4; // Skip data_loss_events
        offset += 1; // Skip retrieval_success_rate
        offset += 4; // Skip slashes

        let reputation_score = u16::from_le_bytes(data[offset..offset + 2].try_into()?);
        offset += 2;

        // Status
        let status = StorageNodeStatus::from_byte(data[offset]);
        offset += 1;

        // Timestamps
        let last_heartbeat = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let last_proof_timestamp = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let pending_rewards = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        let registered_at = i64::from_le_bytes(data[offset..offset + 8].try_into()?);

        Ok(Some(StorageNodeInfo {
            node_id: parsed_node_id,
            owner: parsed_owner,
            stake_amount,
            total_capacity_bytes,
            allocated_capacity_bytes,
            disk_type,
            region,
            bandwidth_mbps,
            status,
            last_heartbeat,
            last_proof_timestamp,
            total_proofs,
            failed_proofs,
            reputation_score,
            pending_rewards,
            registered_at,
        }))
    }

    /// Register a new storage node
    pub async fn register_node(
        &self,
        owner: &Keypair,
        node_id: &str,
        spec: &StorageSpec,
    ) -> Result<Signature> {
        let (registry_pda, _) = self.derive_registry_pda();
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        info!(
            node_id = %node_id,
            owner = %owner.pubkey(),
            capacity_gb = spec.total_capacity_bytes / (1024 * 1024 * 1024),
            region = %spec.region,
            "Registering storage node"
        );

        // Build instruction data
        let discriminator = Self::get_discriminator("register_node");

        let node_id_bytes = node_id.as_bytes();
        let region_bytes = spec.region.as_bytes();

        let data_len = 8 // discriminator
            + 4 + node_id_bytes.len() // node_id
            + 8 // total_capacity_bytes
            + 1 // disk_type
            + 4 + region_bytes.len() // region
            + 4; // bandwidth_mbps

        let mut data = Vec::with_capacity(data_len);

        // Discriminator
        data.extend_from_slice(&discriminator);

        // node_id (Anchor string)
        data.extend_from_slice(&(node_id_bytes.len() as u32).to_le_bytes());
        data.extend_from_slice(node_id_bytes);

        // total_capacity_bytes (u64)
        data.extend_from_slice(&spec.total_capacity_bytes.to_le_bytes());

        // disk_type (u8)
        data.push(spec.disk_type.to_byte());

        // region (Anchor string)
        data.extend_from_slice(&(region_bytes.len() as u32).to_le_bytes());
        data.extend_from_slice(region_bytes);

        // bandwidth_mbps (u32)
        data.extend_from_slice(&spec.bandwidth_mbps.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(registry_pda, false),
                AccountMeta::new(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new_readonly(system_program_id(), false),
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
            node_pda = %node_pda,
            "Node registered successfully"
        );

        Ok(signature)
    }

    /// Add stake to node
    pub async fn add_stake(
        &self,
        owner: &Keypair,
        node_id: &str,
        amount: u64,
    ) -> Result<Signature> {
        let (registry_pda, _) = self.derive_registry_pda();
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        debug!(
            node_id = %node_id,
            amount_cyxwiz = amount / 1_000_000_000,
            "Adding stake to node"
        );

        let discriminator = Self::get_discriminator("add_stake");

        let mut data = Vec::with_capacity(8 + 8);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&amount.to_le_bytes());

        // Get token accounts
        let token_mint = super::types::NodeBlockchainConfig::default().token_mint;
        let owner_token_account = get_associated_token_address(&owner.pubkey(), &token_mint);

        // Registry stake token account
        let registry_token_account = get_associated_token_address(&registry_pda, &token_mint);

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(registry_pda, false),
                AccountMeta::new(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(owner_token_account, false),
                AccountMeta::new(registry_token_account, false),
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
            amount_cyxwiz = amount / 1_000_000_000,
            "Stake added successfully"
        );

        Ok(signature)
    }

    /// Request stake withdrawal (starts 7-day lockup)
    pub async fn request_withdraw(
        &self,
        owner: &Keypair,
        node_id: &str,
        amount: u64,
    ) -> Result<Signature> {
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        debug!(
            node_id = %node_id,
            amount_cyxwiz = amount / 1_000_000_000,
            "Requesting stake withdrawal"
        );

        let discriminator = Self::get_discriminator("request_withdraw");

        let mut data = Vec::with_capacity(8 + 8);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&amount.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
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
            "Withdrawal request submitted, 7-day lockup started"
        );

        Ok(signature)
    }

    /// Complete stake withdrawal (after lockup period)
    pub async fn complete_withdraw(
        &self,
        owner: &Keypair,
        node_id: &str,
    ) -> Result<Signature> {
        let (registry_pda, _) = self.derive_registry_pda();
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        debug!(node_id = %node_id, "Completing stake withdrawal");

        let discriminator = Self::get_discriminator("complete_withdraw");

        let token_mint = super::types::NodeBlockchainConfig::default().token_mint;
        let owner_token_account = get_associated_token_address(&owner.pubkey(), &token_mint);
        let registry_token_account = get_associated_token_address(&registry_pda, &token_mint);

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(registry_pda, false),
                AccountMeta::new(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
                AccountMeta::new(owner_token_account, false),
                AccountMeta::new(registry_token_account, false),
                AccountMeta::new_readonly(spl_token_program_id(), false),
            ],
            data: discriminator.to_vec(),
        };

        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&owner.pubkey()),
            &[owner],
            recent_blockhash,
        );

        let signature = self.rpc_client.send_and_confirm_transaction(&transaction)?;

        info!(signature = %signature, "Withdrawal completed");

        Ok(signature)
    }
}
