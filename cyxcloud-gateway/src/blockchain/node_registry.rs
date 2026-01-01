//! Node Registry Operations
//!
//! Handles StorageNodeRegistry program interactions.

use anyhow::Result;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::types::{
    constants, DiskType, StorageMetrics, StorageNodeInfo, StorageNodeStatus, StorageReputation,
    StorageSpec,
};

/// Get Anchor discriminator for an instruction
fn get_discriminator(instruction_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{}", instruction_name));
    let result = hasher.finalize();
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&result[..8]);
    discriminator
}

/// Node registry operations for the Gateway
pub struct NodeRegistryOps {
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
}

impl NodeRegistryOps {
    /// Create new node registry operations handler
    pub fn new(rpc_client: Arc<RpcClient>, program_id: Pubkey) -> Self {
        Self {
            rpc_client,
            program_id,
        }
    }

    /// Get registry PDA
    pub fn get_registry_pda(&self) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"registry"], &self.program_id)
    }

    /// Get storage node PDA
    pub fn get_node_pda(&self, owner: &Pubkey, node_id: &str) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"storage_node", owner.as_ref(), node_id.as_bytes()],
            &self.program_id,
        )
    }

    /// Get proof PDA
    pub fn get_proof_pda(&self, node: &Pubkey, challenge_id: &[u8]) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"proof", node.as_ref(), challenge_id], &self.program_id)
    }

    /// Get storage node info
    pub async fn get_node(&self, owner: &Pubkey, node_id: &str) -> Result<Option<StorageNodeInfo>> {
        let (node_pda, _) = self.get_node_pda(owner, node_id);

        match self.rpc_client.get_account_data(&node_pda) {
            Ok(data) => {
                if data.len() < 8 {
                    return Ok(None);
                }
                let node = self.parse_node_data(&data[8..], node_id.to_string())?;
                Ok(Some(node))
            }
            Err(_) => Ok(None),
        }
    }

    /// Parse storage node data from raw bytes
    fn parse_node_data(&self, data: &[u8], node_id: String) -> Result<StorageNodeInfo> {
        // StorageNode struct layout is complex - simplified parsing
        // In production, use Anchor's account deserialization
        if data.len() < 200 {
            anyhow::bail!("Node data too short");
        }

        // Parse node_id string (4 bytes length + data)
        let id_len = u32::from_le_bytes(data[0..4].try_into()?) as usize;
        let offset = 4 + id_len;

        // Parse owner
        let owner = Pubkey::try_from(&data[offset..offset + 32])?;
        let offset = offset + 32;

        // Parse stake_amount
        let stake_amount = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        let offset = offset + 8;

        // Parse StorageSpec
        let total_capacity = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        let allocated_capacity = u64::from_le_bytes(data[offset + 8..offset + 16].try_into()?);
        let disk_type = match data[offset + 16] {
            0 => DiskType::HDD,
            1 => DiskType::SSD,
            2 => DiskType::NVMe,
            _ => DiskType::HDD,
        };
        // Skip region string for now
        let offset = offset + 64; // Approximate

        // Parse StorageMetrics
        let bytes_stored = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        let bytes_served = u64::from_le_bytes(data[offset + 8..offset + 16].try_into()?);
        let uptime_seconds = u64::from_le_bytes(data[offset + 16..offset + 24].try_into()?);
        let requests_served = u64::from_le_bytes(data[offset + 24..offset + 32].try_into()?);
        let offset = offset + 32;

        // Parse StorageReputation
        let total_proofs = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
        let failed_proofs = u64::from_le_bytes(data[offset + 8..offset + 16].try_into()?);
        let data_loss_events = u32::from_le_bytes(data[offset + 16..offset + 20].try_into()?);
        let retrieval_success_rate = data[offset + 20];
        let slashes = u32::from_le_bytes(data[offset + 21..offset + 25].try_into()?);
        let reputation_score = u16::from_le_bytes(data[offset + 25..offset + 27].try_into()?);
        let offset = offset + 27;

        // Parse status
        let status = match data[offset] {
            0 => StorageNodeStatus::Active,
            1 => StorageNodeStatus::Inactive,
            2 => StorageNodeStatus::Slashed,
            _ => StorageNodeStatus::Inactive,
        };
        let offset = offset + 1;

        // Parse timestamps
        let last_heartbeat = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        let last_proof_timestamp = i64::from_le_bytes(data[offset + 8..offset + 16].try_into()?);
        let pending_rewards = u64::from_le_bytes(data[offset + 16..offset + 24].try_into()?);
        let registered_at = i64::from_le_bytes(data[offset + 24..offset + 32].try_into()?);

        Ok(StorageNodeInfo {
            node_id,
            owner,
            stake_amount,
            spec: StorageSpec {
                total_capacity_bytes: total_capacity,
                allocated_capacity_bytes: allocated_capacity,
                disk_type,
                region: "unknown".to_string(),
                bandwidth_mbps: 0,
            },
            metrics: StorageMetrics {
                bytes_stored,
                bytes_served,
                uptime_seconds,
                requests_served,
            },
            reputation: StorageReputation {
                total_proofs,
                failed_proofs,
                data_loss_events,
                retrieval_success_rate,
                slashes,
                reputation_score,
            },
            status,
            last_heartbeat,
            last_proof_timestamp,
            pending_rewards,
            registered_at,
        })
    }

    /// Update node metrics (Gateway authority only)
    pub async fn update_metrics(
        &self,
        authority: &Keypair,
        owner: &Pubkey,
        node_id: &str,
        metrics: &StorageMetrics,
    ) -> Result<String> {
        let (registry_pda, _) = self.get_registry_pda();
        let (node_pda, _) = self.get_node_pda(owner, node_id);

        // Build instruction data
        let discriminator = get_discriminator("update_metrics");
        let mut ix_data = Vec::with_capacity(40);
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&metrics.bytes_stored.to_le_bytes());
        ix_data.extend_from_slice(&metrics.bytes_served.to_le_bytes());
        ix_data.extend_from_slice(&metrics.uptime_seconds.to_le_bytes());
        ix_data.extend_from_slice(&metrics.requests_served.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new_readonly(registry_pda, false),
                AccountMeta::new(node_pda, false),
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
            node_id = node_id,
            owner = %owner,
            signature = %signature,
            "Node metrics updated"
        );

        Ok(signature.to_string())
    }

    /// Slash a node (Gateway authority only)
    pub async fn slash_node(
        &self,
        authority: &Keypair,
        owner: &Pubkey,
        node_id: &str,
        amount: u64,
        reason: &str,
    ) -> Result<String> {
        let (registry_pda, _) = self.get_registry_pda();
        let (node_pda, _) = self.get_node_pda(owner, node_id);

        // Build instruction data
        let discriminator = get_discriminator("slash_node");
        let mut ix_data = Vec::with_capacity(16 + reason.len());
        ix_data.extend_from_slice(&discriminator);
        ix_data.extend_from_slice(&amount.to_le_bytes());
        // Reason string (4-byte length prefix + data)
        ix_data.extend_from_slice(&(reason.len() as u32).to_le_bytes());
        ix_data.extend_from_slice(reason.as_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(registry_pda, false),
                AccountMeta::new(node_pda, false),
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
        warn!(
            node_id = node_id,
            owner = %owner,
            amount = amount,
            reason = reason,
            signature = %signature,
            "Node slashed"
        );

        Ok(signature.to_string())
    }

    /// Check if node is active and has valid heartbeat
    pub async fn is_node_active(&self, owner: &Pubkey, node_id: &str) -> Result<bool> {
        match self.get_node(owner, node_id).await? {
            Some(node) => {
                let now = chrono::Utc::now().timestamp();
                let heartbeat_valid =
                    now - node.last_heartbeat < constants::HEARTBEAT_TIMEOUT_SECONDS;
                Ok(node.status == StorageNodeStatus::Active && heartbeat_valid)
            }
            None => Ok(false),
        }
    }

    /// Get all active storage nodes
    pub async fn get_active_nodes(&self) -> Result<Vec<StorageNodeInfo>> {
        // In production, this would use getProgramAccounts with filters
        // For now, return empty list - nodes should be tracked in local database
        debug!("get_active_nodes: Would query on-chain data");
        Ok(Vec::new())
    }

    /// Calculate reputation score for a node
    pub fn calculate_reputation_score(reputation: &StorageReputation) -> u16 {
        let total = reputation.total_proofs;
        if total == 0 {
            return 5000; // Starting score
        }

        // Base score from proof success rate (0-6000 points)
        let success_rate = if total > 0 {
            ((total - reputation.failed_proofs) * 6000 / total) as u16
        } else {
            3000
        };

        // Retrieval rate bonus (0-2000 points)
        let retrieval_bonus = (reputation.retrieval_success_rate as u16) * 20;

        // Penalty for data loss events (up to -2000 points)
        let data_loss_penalty = (reputation.data_loss_events as u16).min(20) * 100;

        // Penalty for slashes (up to -2000 points)
        let slash_penalty = (reputation.slashes as u16).min(10) * 200;

        let score = success_rate
            .saturating_add(retrieval_bonus)
            .saturating_sub(data_loss_penalty)
            .saturating_sub(slash_penalty);

        score.min(10000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_pda() {
        let program_id = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let node_id = "test-node-001";
        let rpc = Arc::new(RpcClient::new("http://localhost:8899".to_string()));
        let ops = NodeRegistryOps::new(rpc, program_id);

        let (pda, bump) = ops.get_node_pda(&owner, node_id);
        assert!(bump <= 255);
        assert_ne!(pda, owner);
    }

    #[test]
    fn test_reputation_score() {
        let good_reputation = StorageReputation {
            total_proofs: 1000,
            failed_proofs: 10,
            data_loss_events: 0,
            retrieval_success_rate: 99,
            slashes: 0,
            reputation_score: 0,
        };

        let score = NodeRegistryOps::calculate_reputation_score(&good_reputation);
        assert!(score > 7000); // Should have high score

        let bad_reputation = StorageReputation {
            total_proofs: 100,
            failed_proofs: 50,
            data_loss_events: 5,
            retrieval_success_rate: 50,
            slashes: 3,
            reputation_score: 0,
        };

        let score = NodeRegistryOps::calculate_reputation_score(&bad_reputation);
        assert!(score < 5000); // Should have low score
    }
}
