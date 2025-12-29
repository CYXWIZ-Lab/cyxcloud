# CyxCloud Node Payment System

## Overview

Storage nodes are paid weekly based on their contribution to the network. Payment is pro-rated by uptime, weighted by storage capacity and reputation, with penalties (slashing) for misbehavior.

## Payment Configuration

| Setting | Value | Description |
|---------|-------|-------------|
| Payment Cycle | 7 days | Weekly epochs |
| Node Share | 85% | Portion of pool going to storage nodes |
| Platform Fee | 10% | Gateway/platform maintenance fee |
| Community Fund | 5% | Reserved for community initiatives |
| Heartbeat Timeout | 5 minutes | Time without heartbeat before marking offline |

## Payment Formula

### Weight Calculation

Each node's share is determined by their **weight**:

```
weight = storage_bytes × uptime_factor × reputation_factor
```

Where:
- **storage_bytes**: Total storage the node is providing
- **uptime_factor**: `seconds_online / 604800` (capped at 1.0)
- **reputation_factor**: `0.5 + (reputation_score / 10000)` (range 0.5 - 1.5)

### Node Share Calculation

```
node_reward = (total_pool × 85%) × (node_weight / sum_of_all_weights)
```

### Examples

**Node A**: 1TB storage, 100% uptime, 10000 reputation (perfect)
```
uptime_factor = 1.0
reputation_factor = 0.5 + (10000/10000) = 1.5
weight = 1,000,000,000,000 × 1.0 × 1.5 = 1,500,000,000,000
```

**Node B**: 500GB storage, 50% uptime (3.5 days offline), 5000 reputation (average)
```
uptime_factor = 0.5
reputation_factor = 0.5 + (5000/10000) = 1.0
weight = 500,000,000,000 × 0.5 × 1.0 = 250,000,000,000
```

**Distribution** (with 1000 CYXWIZ in pool):
- Nodes share: 850 CYXWIZ (85%)
- Total weight: 1,750,000,000,000
- Node A: 850 × (1.5T / 1.75T) = **728.57 CYXWIZ**
- Node B: 850 × (0.25T / 1.75T) = **121.43 CYXWIZ**

## Epoch Lifecycle

```
Day 0: Epoch Starts
├── Gateway initializes epoch tracking
├── All online nodes begin accumulating uptime
└── Payment daemon starts monitoring

Days 1-6: Ongoing Monitoring
├── Heartbeats tracked every 30 seconds
├── Uptime accumulated every minute
├── Status transitions recorded
└── Slashing events detected and queued

Day 7: Epoch Ends
├── Calculate final uptime per node
├── Check for slashing conditions
├── Execute any pending slashes on-chain
├── Calculate node weights
├── Allocate rewards proportionally on Solana
├── Finalize epoch on-chain
├── Claim platform share (10%)
├── Claim community share (5%)
└── Start new epoch
```

## Payment Scenarios

### Normal Operation
- Node online all 7 days → 100% of calculated share
- Consistent heartbeats (every 30 seconds)

### Disconnection / Maintenance
- Node offline for 3 days → 4/7 (57%) of calculated share
- Uptime tracked in seconds for precise calculation
- No penalty for planned maintenance (just reduced payment)

### Extended Downtime (>4 hours)
- Node still receives pro-rated payment for online time
- **5% slash** on staked tokens as penalty
- Reputation score reduced

### Drive Failure / Data Loss
- If chunks are lost due to node failure:
  - **10% slash** on staked tokens
  - Reputation significantly reduced
  - Rebalancer redistributes lost chunks to healthy nodes

### Corrupted Data
- If node serves corrupted data (verification failures):
  - **50% slash** on staked tokens
  - Node may be removed from network
  - Affected chunks repaired from parity

### Failed Proofs
- If node fails storage proofs:
  - **15% slash** on staked tokens
  - Investigation triggered for potential fraud

## Slashing Penalties

| Reason | Slash % | Description |
|--------|---------|-------------|
| Extended Downtime | 5% | Offline > 4 hours in epoch |
| Data Loss | 10% | Chunks permanently lost |
| Failed Proofs | 15% | Failed to prove storage |
| Corrupted Data | 50% | Served invalid data |

## Database Schema

### node_epoch_uptime
Tracks uptime per node per epoch:
- `seconds_online`: Accumulated online time
- `seconds_offline`: Accumulated offline time
- `payment_amount`: Calculated reward
- `payment_allocated`: Whether payment was sent on-chain

### node_slashing_events
Records slashing penalties:
- `reason`: Type of violation
- `slash_percent`: Percentage slashed
- `slash_amount`: Token amount slashed
- `tx_signature`: Solana transaction

### payment_epochs
Tracks epoch state:
- `total_pool_amount`: Total tokens in pool
- `nodes_share`: 85% allocated to nodes
- `nodes_paid`: Count of nodes paid
- `finalized`: Whether epoch is closed

## Solana Integration

Payment uses three Solana programs:

1. **StoragePaymentPool** (`4wFUJ1SVpVDEpYTobgkLSnwY2yXr4LKAMdWcG3sAe3sX`)
   - Manages epoch lifecycle
   - Distributes rewards
   - Tracks claims

2. **StorageNodeRegistry** (`AQPP8YaiGazv9Mh4bnsVcGiMgTKbarZeCcQsh4jmpi4Z`)
   - Node staking
   - Slashing execution
   - Reputation tracking

3. **CYXWIZ Token** (`Az2YZ1hmY5iQ6Gi9rjTPRpNMvcyeYVt1PqjyRSRoNNYi`)
   - SPL token for payments
   - 9 decimal places

## Payment Daemon

The gateway runs a background `PaymentDaemon` that:

1. **Every minute**: Accumulates uptime for all nodes based on current status
2. **Every epoch end (7 days)**:
   - Calculates weights for all nodes
   - Checks for slashing conditions
   - Executes slashes on Solana
   - Allocates rewards on Solana
   - Finalizes epoch
   - Starts new epoch

## API Endpoints

(Future implementation)

- `GET /api/v1/payment/epochs` - List epoch history
- `GET /api/v1/payment/epochs/:epoch/nodes` - Node rewards for epoch
- `GET /api/v1/payment/nodes/:node_id/history` - Payment history for node
- `GET /api/v1/payment/pool` - Current pool status

## Testing

1. Start gateway with `--enable-blockchain` flag
2. Set environment variables:
   - `SOLANA_RPC_URL=https://api.devnet.solana.com`
   - `GATEWAY_KEYPAIR_PATH=/path/to/keypair.json`
3. Register test nodes with Solana wallets
4. Run nodes for testing period
5. Verify uptime tracking in database
6. Check reward calculations match expected values
7. Verify Solana transactions
