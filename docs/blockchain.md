# CyxCloud Blockchain Architecture - Implementation Plan

## Overview

Integrate Solana blockchain with CYXWIZ tokens into CyxCloud distributed storage system. This enables on-chain payments where the Gateway coordinates but never custodies funds.

**CyxCloud is part of the unified CyxWiz ecosystem**, sharing the same CYXWIZ token and blockchain infrastructure with the distributed ML compute platform.

**Key Decisions:**
- Minimum stake: **500 CYXWIZ**
- Payment epoch: **Weekly**
- Proof-of-Storage: **Phase 1** (full implementation)

---

## Ecosystem Configuration

CyxCloud uses the same Solana keypair and token as the CyxWiz distributed compute platform:

| Setting | Value |
|---------|-------|
| **Network** | Solana Devnet (mainnet after launch) |
| **RPC URL** | `https://api.devnet.solana.com` |
| **Keypair Path** | `~/.config/solana/id.json` |
| **Authority Pubkey** | `4Y5HWB9W9SELq3Yoyf7mK7KF5kTbuaGxd2BMvn3AyAG8` |
| **Token** | CYXWIZ (9 decimals) |

### Environment Variables

```bash
# Solana Configuration
SOLANA_RPC_URL=https://api.devnet.solana.com
GATEWAY_KEYPAIR_PATH=~/.config/solana/id.json

# Program IDs (update after deployment)
SUBSCRIPTION_PROGRAM_ID=StoS111111111111111111111111111111111111111
NODE_REGISTRY_PROGRAM_ID=StoN111111111111111111111111111111111111111
PAYMENT_POOL_PROGRAM_ID=StoP111111111111111111111111111111111111111

# Token Configuration
CYXWIZ_TOKEN_MINT=<token_mint_after_deployment>
PLATFORM_TREASURY=<platform_treasury_token_account>
COMMUNITY_FUND=<community_fund_token_account>
```

---

## Architecture Summary

### Three New Anchor Programs

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    CyxCloud Blockchain Programs                          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────────────┐  ┌─────────────────────┐  ┌──────────────────┐ │
│  │ StorageSubscription │  │ StorageNodeRegistry │  │ StoragePaymentPool│ │
│  │                     │  │                     │  │                   │ │
│  │ • User storage plans│  │ • Node registration │  │ • Collect payments│ │
│  │ • Subscription mgmt │  │ • Staking (500 CYXW)│  │ • Weekly epochs   │ │
│  │ • Usage tracking    │  │ • Proof-of-Storage  │  │ • 85/10/5 split   │ │
│  │ • Auto-renewal      │  │ • Reputation/Slash  │  │ • Reward claims   │ │
│  └─────────────────────┘  └─────────────────────┘  └──────────────────┘ │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

### Payment Distribution (Weekly)
- **85%** → Storage Nodes (proportional to storage + uptime + speed)
- **10%** → Platform Treasury
- **5%** → Community Fund

---

## Implementation Phases

### Phase 1: Core Programs (Week 1-2)

#### 1.1 StoragePaymentPool Program
**File:** `cyxwiz-blockchain/programs/storage-payment-pool/src/lib.rs`

```rust
// Key Account
pub struct PaymentPool {
    pub authority: Pubkey,           // Gateway authority
    pub total_deposits: u64,
    pub current_epoch: u64,
    pub epoch_duration_seconds: i64, // 604800 (7 days)
    pub distribution_config: DistributionConfig, // 85/10/5
    pub bump: u8,
}

pub struct EpochRewards {
    pub epoch: u64,
    pub total_pool_amount: u64,
    pub distributed_amount: u64,
    pub finalized: bool,
    pub bump: u8,
}

// Instructions
- initialize_pool()
- deposit_subscription_payment()
- start_new_epoch()
- calculate_node_rewards()
- claim_node_rewards()
- distribute_platform_share()
- distribute_community_share()
```

#### 1.2 StorageSubscription Program
**File:** `cyxwiz-blockchain/programs/storage-subscription/src/lib.rs`

```rust
// Key Account
pub struct StorageSubscription {
    pub user: Pubkey,
    pub plan_type: StoragePlanType,  // Free/Starter/Pro/Enterprise
    pub storage_quota_bytes: u64,
    pub storage_used_bytes: u64,
    pub price_per_period: u64,
    pub period_type: PeriodType,     // Monthly/Yearly
    pub current_period_end: i64,
    pub auto_renew: bool,
    pub status: SubscriptionStatus,  // Active/Expired/Cancelled
    pub bump: u8,
}

// PDA: [b"subscription", user.key().as_ref()]

// Instructions
- create_subscription(plan_type, period_type)
- renew_subscription()
- upgrade_plan(new_plan)
- cancel_subscription()
- update_usage(storage_used, bandwidth_used)  // Gateway only
```

#### 1.3 StorageNodeRegistry Program
**File:** `cyxwiz-blockchain/programs/storage-node-registry/src/lib.rs`

```rust
// Key Account
pub struct StorageNode {
    pub node_id: String,             // Max 64 chars
    pub owner: Pubkey,
    pub stake_amount: u64,           // Min 500 CYXWIZ
    pub storage_spec: StorageSpec,
    pub metrics: StorageMetrics,
    pub reputation: StorageReputation,
    pub status: StorageNodeStatus,   // Active/Inactive/Slashed
    pub last_heartbeat: i64,
    pub last_proof_timestamp: i64,
    pub pending_rewards: u64,
    pub bump: u8,
}

pub struct StorageSpec {
    pub total_capacity_bytes: u64,
    pub allocated_capacity_bytes: u64,
    pub disk_type: DiskType,         // SSD/HDD/NVMe
    pub region: String,
    pub bandwidth_mbps: u32,
}

pub struct StorageReputation {
    pub total_proofs: u64,
    pub failed_proofs: u64,
    pub data_loss_events: u32,
    pub retrieval_success_rate: u8,  // 0-100
    pub slashes: u32,
    pub reputation_score: u16,       // 0-10000
}

// PDA: [b"storage_node", owner.key().as_ref(), node_id.as_bytes()]

// Instructions
- register_storage_node(node_id, stake_amount, storage_spec)
- heartbeat()
- submit_proof_of_storage(challenge, proof)
- update_metrics(metrics)  // Gateway only
- add_stake(amount)
- request_withdraw(amount)
- complete_withdraw()
- claim_rewards(epoch)
- slash_node(amount, reason)  // Gateway only
```

### Phase 2: Gateway Integration (Week 2-3)

**New Files in `cyx_cloud/cyxcloud-gateway/src/blockchain/`:**

```
blockchain/
├── mod.rs              # Module exports
├── client.rs           # CyxCloudBlockchainClient
├── subscription.rs     # Subscription operations
├── node_registry.rs    # Node registry operations
├── payment_pool.rs     # Payment pool operations
└── types.rs            # Shared types
```

**Key Integration:**
```rust
// cyxcloud-gateway/src/blockchain/client.rs
pub struct CyxCloudBlockchainClient {
    rpc_client: Arc<RpcClient>,
    gateway_authority: Arc<Keypair>,
    subscription_program_id: Pubkey,
    storage_registry_program_id: Pubkey,
    payment_pool_program_id: Pubkey,
    token_mint: Pubkey,  // CYXWIZ
}

impl CyxCloudBlockchainClient {
    // Subscription management
    pub async fn create_subscription(...) -> Result<Signature>;
    pub async fn update_usage(...) -> Result<Signature>;

    // Node management
    pub async fn update_node_metrics(...) -> Result<Signature>;
    pub async fn slash_node(...) -> Result<Signature>;

    // Payment epochs
    pub async fn start_new_epoch() -> Result<Signature>;
    pub async fn calculate_epoch_rewards(epoch: u64) -> Result<Signature>;
}
```

### Phase 3: Storage Node Integration (Week 3)

**New Files in `cyx_cloud/cyxcloud-node/src/blockchain/`:**

```
blockchain/
├── mod.rs              # Module exports
├── client.rs           # StorageNodeBlockchainClient
├── registration.rs     # Node registration
├── heartbeat.rs        # Heartbeat with proof
└── rewards.rs          # Reward claiming
```

**Key Integration:**
```rust
// cyxcloud-node/src/blockchain/client.rs
pub struct StorageNodeBlockchainClient {
    rpc_client: Arc<RpcClient>,
    node_owner: Arc<Keypair>,
    storage_registry_program_id: Pubkey,
    payment_pool_program_id: Pubkey,
}

impl StorageNodeBlockchainClient {
    pub async fn register_node(...) -> Result<Signature>;
    pub async fn send_heartbeat() -> Result<Signature>;
    pub async fn submit_proof_of_storage(...) -> Result<Signature>;
    pub async fn claim_epoch_rewards(epoch: u64) -> Result<Signature>;
}
```

### Phase 4: Website Integration (Week 4)

**New Files in `cyxwiz_web/apps/web/src/lib/blockchain/`:**

```
blockchain/
├── storage-subscription.ts   # Subscription client
├── storage-pricing.ts        # Pricing constants
└── hooks/
    ├── useStorageSubscription.ts
    └── useStorageBalance.ts
```

---

## File Structure Summary

### New Files to Create

```
cyxwiz-blockchain/
├── programs/
│   ├── storage-subscription/           # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── state.rs
│   │       ├── instructions/
│   │       │   ├── mod.rs
│   │       │   ├── create_subscription.rs
│   │       │   ├── renew_subscription.rs
│   │       │   ├── upgrade_plan.rs
│   │       │   ├── cancel_subscription.rs
│   │       │   └── update_usage.rs
│   │       └── errors.rs
│   ├── storage-node-registry/          # NEW
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── state.rs
│   │       ├── instructions/
│   │       │   ├── mod.rs
│   │       │   ├── register_node.rs
│   │       │   ├── heartbeat.rs
│   │       │   ├── submit_proof.rs
│   │       │   ├── update_metrics.rs
│   │       │   ├── stake_operations.rs
│   │       │   ├── claim_rewards.rs
│   │       │   └── slash_node.rs
│   │       └── errors.rs
│   └── storage-payment-pool/           # NEW
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── state.rs
│           ├── instructions/
│           │   ├── mod.rs
│           │   ├── initialize_pool.rs
│           │   ├── deposit_payment.rs
│           │   ├── epoch_management.rs
│           │   ├── calculate_rewards.rs
│           │   ├── claim_rewards.rs
│           │   └── distribute_shares.rs
│           └── errors.rs
├── tests/
│   ├── storage-subscription.ts         # NEW
│   ├── storage-node-registry.ts        # NEW
│   └── storage-payment-pool.ts         # NEW
└── Anchor.toml                         # UPDATE with new programs

cyx_cloud/cyxcloud-gateway/src/
└── blockchain/                         # NEW directory
    ├── mod.rs
    ├── client.rs
    ├── subscription.rs
    ├── node_registry.rs
    ├── payment_pool.rs
    └── types.rs

cyx_cloud/cyxcloud-node/src/
└── blockchain/                         # NEW directory
    ├── mod.rs
    ├── client.rs
    ├── registration.rs
    ├── heartbeat.rs
    └── rewards.rs
```

### Files to Modify

```
cyxwiz-blockchain/Anchor.toml          # Add new program IDs
cyxwiz-blockchain/Cargo.toml           # Add workspace members

cyx_cloud/cyxcloud-gateway/Cargo.toml  # Add solana-sdk, anchor-client
cyx_cloud/cyxcloud-gateway/src/main.rs # Initialize blockchain client
cyx_cloud/cyxcloud-gateway/src/state.rs # Add blockchain client to AppState

cyx_cloud/cyxcloud-node/Cargo.toml     # Add solana-sdk, anchor-client
cyx_cloud/cyxcloud-node/src/main.rs    # Initialize blockchain client
cyx_cloud/cyxcloud-node/src/config.rs  # Add [blockchain] config section
```

---

## Constants

```rust
// Staking
pub const MIN_STORAGE_STAKE: u64 = 500_000_000_000; // 500 CYXWIZ (9 decimals)
pub const UNSTAKE_LOCKUP_SECONDS: i64 = 604_800;    // 7 days

// Payment epochs
pub const EPOCH_DURATION_SECONDS: i64 = 604_800;    // 7 days (weekly)

// Distribution
pub const STORAGE_NODE_PERCENT: u8 = 85;
pub const PLATFORM_TREASURY_PERCENT: u8 = 10;
pub const COMMUNITY_FUND_PERCENT: u8 = 5;

// Heartbeat
pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 300;     // 5 minutes

// Slashing percentages
pub const SLASH_DATA_LOSS: u8 = 10;                 // 10% stake
pub const SLASH_EXTENDED_DOWNTIME: u8 = 5;          // 5% stake
pub const SLASH_CORRUPTED_DATA: u8 = 50;            // 50% stake
pub const SLASH_FAILED_PROOFS: u8 = 15;             // 15% stake

// Pricing (CYXWIZ with 9 decimals)
pub const PRICE_STARTER_MONTHLY: u64 = 10_000_000_000;    // 10 CYXWIZ
pub const PRICE_PRO_MONTHLY: u64 = 50_000_000_000;        // 50 CYXWIZ
pub const PRICE_ENTERPRISE_MONTHLY: u64 = 200_000_000_000; // 200 CYXWIZ
```

---

## Payment Flow

```
User purchases storage subscription
         │
         ▼
┌─────────────────────────────────────┐
│  StorageSubscription.create_subscription()  │
│  - CYXWIZ transferred to PaymentPool        │
│  - Subscription PDA created                 │
└─────────────────────────────────────┘
         │
         ▼
PaymentPool accumulates weekly payments
         │
         ▼ (Every Sunday)
┌─────────────────────────────────────┐
│  Gateway calls start_new_epoch()            │
│  - Snapshot all node metrics                │
│  - Calculate weighted rewards               │
│  - Create NodeEpochClaim for each node      │
└─────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│  Distribution:                              │
│  85% → Storage Nodes (claim individually)   │
│  10% → Platform Treasury                    │
│   5% → Community Fund                       │
└─────────────────────────────────────┘
         │
         ▼
Nodes call claim_rewards(epoch) to receive CYXWIZ
```

---

## Proof-of-Storage Design

```
Gateway                    Storage Node                 Blockchain
   │                            │                            │
   │── 1. Random challenge ────▶│                            │
   │   (chunk_id, nonce)        │                            │
   │                            │                            │
   │                            │── 2. Compute proof ────────│
   │                            │   hash(chunk_data + nonce) │
   │                            │                            │
   │                            │── 3. submit_proof() ──────▶│
   │                            │   (challenge, proof)       │
   │                            │                            │
   │◀── 4. Verify proof ────────│◀── proof_result ──────────│
   │                            │                            │
   │   If valid: update reputation.total_proofs++           │
   │   If invalid: reputation.failed_proofs++               │
   │   If 3 consecutive fails: trigger slash                │
```

---

## Implementation Order

1. **StoragePaymentPool** - Central to payment flow, needed first
2. **StorageSubscription** - Enables user payments to pool
3. **StorageNodeRegistry** - Enables nodes to register and earn
4. **Gateway blockchain client** - Coordinate all operations
5. **Node blockchain client** - Node registration and claims
6. **Tests** - Comprehensive test suite
7. **Deploy to devnet** - Test full flow
8. **Website integration** - User-facing subscription UI

---

## Dependencies to Add

**cyxcloud-gateway/Cargo.toml:**
```toml
solana-sdk = "1.17"
solana-client = "1.17"
anchor-client = "0.29"
```

**cyxcloud-node/Cargo.toml:**
```toml
solana-sdk = "1.17"
solana-client = "1.17"
anchor-client = "0.29"
```

---

## Success Criteria

- [ ] All 3 Anchor programs compile and pass tests
- [ ] Programs deployed to Solana devnet
- [ ] Gateway can create subscriptions and manage epochs
- [ ] Storage nodes can register, stake, and claim rewards
- [ ] Weekly epoch distribution works correctly (85/10/5 split)
- [ ] Proof-of-storage challenges work with slashing
- [ ] Website can initiate subscriptions via wallet
