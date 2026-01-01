//! Heartbeat and Proof-of-Storage Operations
//!
//! Handles periodic heartbeats and proof-of-storage challenges.

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
use tracing::{debug, info, warn};

use super::types::{ProofChallenge, ProofOfStorage};

/// Heartbeat operations for storage nodes
pub struct HeartbeatOps {
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
}

impl HeartbeatOps {
    /// Create new heartbeat operations handler
    pub fn new(rpc_client: Arc<RpcClient>, program_id: Pubkey) -> Self {
        Self {
            rpc_client,
            program_id,
        }
    }

    /// Derive the storage node PDA
    fn derive_node_pda(&self, owner: &Pubkey, node_id: &str) -> (Pubkey, u8) {
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

    /// Send heartbeat to update last_heartbeat timestamp
    pub async fn heartbeat(&self, owner: &Keypair, node_id: &str) -> Result<Signature> {
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        debug!(node_id = %node_id, "Sending heartbeat");

        let discriminator = Self::get_discriminator("heartbeat");

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(node_pda, false),
                AccountMeta::new(owner.pubkey(), true),
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

        debug!(signature = %signature, "Heartbeat sent");

        Ok(signature)
    }

    /// Submit proof-of-storage response
    pub async fn submit_proof(
        &self,
        owner: &Keypair,
        node_id: &str,
        proof: &ProofOfStorage,
    ) -> Result<Signature> {
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        info!(
            node_id = %node_id,
            challenge_id = proof.challenge_id,
            "Submitting proof-of-storage"
        );

        let discriminator = Self::get_discriminator("submit_proof_of_storage");

        // Build instruction data
        let mut data = Vec::with_capacity(8 + 8 + 32);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&proof.challenge_id.to_le_bytes());
        data.extend_from_slice(&proof.proof_hash);

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
            challenge_id = proof.challenge_id,
            "Proof submitted successfully"
        );

        Ok(signature)
    }

    /// Generate proof-of-storage from chunk data and challenge
    pub fn generate_proof(chunk_data: &[u8], challenge: &ProofChallenge) -> ProofOfStorage {
        // Hash: SHA256(chunk_data || nonce)
        let mut hasher = Sha256::new();
        hasher.update(chunk_data);
        hasher.update(&challenge.nonce);
        let result = hasher.finalize();

        let mut proof_hash = [0u8; 32];
        proof_hash.copy_from_slice(&result);

        ProofOfStorage {
            challenge_id: challenge.challenge_id,
            proof_hash,
            merkle_path: None,
        }
    }

    /// Update node metrics on-chain
    pub async fn update_metrics(
        &self,
        owner: &Keypair,
        node_id: &str,
        bytes_stored: u64,
        bytes_served: u64,
        uptime_seconds: u64,
        requests_served: u64,
    ) -> Result<Signature> {
        let (node_pda, _) = self.derive_node_pda(&owner.pubkey(), node_id);

        debug!(
            node_id = %node_id,
            bytes_stored = bytes_stored,
            requests = requests_served,
            "Updating metrics"
        );

        let discriminator = Self::get_discriminator("update_metrics");

        // Build instruction data
        let mut data = Vec::with_capacity(8 + 4 * 8);
        data.extend_from_slice(&discriminator);
        data.extend_from_slice(&bytes_stored.to_le_bytes());
        data.extend_from_slice(&bytes_served.to_le_bytes());
        data.extend_from_slice(&uptime_seconds.to_le_bytes());
        data.extend_from_slice(&requests_served.to_le_bytes());

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

        debug!(signature = %signature, "Metrics updated");

        Ok(signature)
    }
}

/// Heartbeat service that runs in the background
pub struct BlockchainHeartbeatService {
    ops: HeartbeatOps,
    owner: Arc<Keypair>,
    node_id: String,
    interval_secs: u64,
}

impl BlockchainHeartbeatService {
    /// Create a new heartbeat service
    pub fn new(
        ops: HeartbeatOps,
        owner: Arc<Keypair>,
        node_id: String,
        interval_secs: u64,
    ) -> Self {
        Self {
            ops,
            owner,
            node_id,
            interval_secs,
        }
    }

    /// Run the heartbeat service (blocking)
    pub async fn run(&self) {
        info!(
            node_id = %self.node_id,
            interval_secs = self.interval_secs,
            "Starting blockchain heartbeat service"
        );

        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(self.interval_secs));

        loop {
            interval.tick().await;

            match self.ops.heartbeat(&self.owner, &self.node_id).await {
                Ok(sig) => {
                    debug!(signature = %sig, "Blockchain heartbeat sent");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to send blockchain heartbeat");
                }
            }
        }
    }
}
