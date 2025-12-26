//! Subscription Operations
//!
//! Handles StorageSubscription program interactions.

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::sync::Arc;
use tracing::{debug, info};

use super::types::{PeriodType, StoragePlanType, SubscriptionInfo, SubscriptionStatus};

/// Subscription operations for the Gateway
pub struct SubscriptionOps {
    rpc_client: Arc<RpcClient>,
    program_id: Pubkey,
    token_mint: Pubkey,
}

impl SubscriptionOps {
    /// Create new subscription operations handler
    pub fn new(rpc_client: Arc<RpcClient>, program_id: Pubkey, token_mint: Pubkey) -> Self {
        Self {
            rpc_client,
            program_id,
            token_mint,
        }
    }

    /// Get subscription PDA for a user
    pub fn get_subscription_pda(&self, user: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"subscription", user.as_ref()],
            &self.program_id,
        )
    }

    /// Get subscription info for a user
    pub async fn get_subscription(&self, user: &Pubkey) -> Result<Option<SubscriptionInfo>> {
        let (subscription_pda, _) = self.get_subscription_pda(user);

        // Fetch account data
        match self.rpc_client.get_account_data(&subscription_pda) {
            Ok(data) => {
                // Skip 8-byte discriminator
                if data.len() < 8 {
                    return Ok(None);
                }

                // Parse subscription data
                // The layout matches our Anchor struct
                let subscription = self.parse_subscription_data(&data[8..], *user)?;
                Ok(Some(subscription))
            }
            Err(_) => Ok(None),
        }
    }

    /// Parse subscription data from raw bytes
    fn parse_subscription_data(&self, data: &[u8], user: Pubkey) -> Result<SubscriptionInfo> {
        // Subscription struct layout:
        // user: Pubkey (32)
        // plan_type: u8 (1)
        // period_type: u8 (1)
        // storage_quota_bytes: u64 (8)
        // storage_used_bytes: u64 (8)
        // bandwidth_used_bytes: u64 (8)
        // price_per_period: u64 (8)
        // current_period_start: i64 (8)
        // current_period_end: i64 (8)
        // auto_renew: bool (1)
        // status: u8 (1)
        // bump: u8 (1)

        if data.len() < 85 {
            anyhow::bail!("Subscription data too short");
        }

        let plan_type = match data[32] {
            0 => StoragePlanType::Free,
            1 => StoragePlanType::Starter,
            2 => StoragePlanType::Pro,
            3 => StoragePlanType::Enterprise,
            _ => anyhow::bail!("Unknown plan type"),
        };

        let period_type = match data[33] {
            0 => PeriodType::Monthly,
            1 => PeriodType::Yearly,
            _ => anyhow::bail!("Unknown period type"),
        };

        let storage_quota_bytes = u64::from_le_bytes(data[34..42].try_into()?);
        let storage_used_bytes = u64::from_le_bytes(data[42..50].try_into()?);
        let bandwidth_used_bytes = u64::from_le_bytes(data[50..58].try_into()?);
        let price_per_period = u64::from_le_bytes(data[58..66].try_into()?);
        let current_period_start = i64::from_le_bytes(data[66..74].try_into()?);
        let current_period_end = i64::from_le_bytes(data[74..82].try_into()?);
        let auto_renew = data[82] != 0;

        let status = match data[83] {
            0 => SubscriptionStatus::Active,
            1 => SubscriptionStatus::Expired,
            2 => SubscriptionStatus::Cancelled,
            _ => anyhow::bail!("Unknown subscription status"),
        };

        Ok(SubscriptionInfo {
            user,
            plan_type,
            period_type,
            storage_quota_bytes,
            storage_used_bytes,
            bandwidth_used_bytes,
            price_per_period,
            current_period_start,
            current_period_end,
            auto_renew,
            status,
        })
    }

    /// Update usage for a subscription (Gateway authority only)
    pub async fn update_usage(
        &self,
        authority: &Keypair,
        user: &Pubkey,
        storage_used: u64,
        bandwidth_used: u64,
    ) -> Result<String> {
        let (subscription_pda, _) = self.get_subscription_pda(user);

        // Build instruction data
        // Discriminator for update_usage + storage_used (u64) + bandwidth_used (u64)
        let mut ix_data = Vec::with_capacity(24);
        // Anchor discriminator for "update_usage" (first 8 bytes of sha256("global:update_usage"))
        ix_data.extend_from_slice(&[195, 135, 242, 26, 146, 223, 93, 159]); // placeholder - calculate actual
        ix_data.extend_from_slice(&storage_used.to_le_bytes());
        ix_data.extend_from_slice(&bandwidth_used.to_le_bytes());

        let instruction = Instruction {
            program_id: self.program_id,
            accounts: vec![
                AccountMeta::new(subscription_pda, false),
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
            user = %user,
            storage = storage_used,
            bandwidth = bandwidth_used,
            signature = %signature,
            "Usage updated"
        );

        Ok(signature.to_string())
    }

    /// Check if subscription is active
    pub async fn is_active(&self, user: &Pubkey) -> Result<bool> {
        match self.get_subscription(user).await? {
            Some(sub) => Ok(sub.status == SubscriptionStatus::Active),
            None => Ok(false),
        }
    }

    /// Get remaining storage quota for a user
    pub async fn get_remaining_quota(&self, user: &Pubkey) -> Result<u64> {
        match self.get_subscription(user).await? {
            Some(sub) => {
                if sub.storage_quota_bytes > sub.storage_used_bytes {
                    Ok(sub.storage_quota_bytes - sub.storage_used_bytes)
                } else {
                    Ok(0)
                }
            }
            None => Ok(0),
        }
    }

    /// Check if user has enough quota for additional storage
    pub async fn has_quota(&self, user: &Pubkey, additional_bytes: u64) -> Result<bool> {
        let remaining = self.get_remaining_quota(user).await?;
        Ok(remaining >= additional_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscription_pda() {
        let program_id = Pubkey::new_unique();
        let user = Pubkey::new_unique();
        let rpc = Arc::new(RpcClient::new("http://localhost:8899".to_string()));
        let ops = SubscriptionOps::new(rpc, program_id, Pubkey::new_unique());

        let (pda, bump) = ops.get_subscription_pda(&user);
        assert!(bump <= 255);
        assert_ne!(pda, user);
    }
}
