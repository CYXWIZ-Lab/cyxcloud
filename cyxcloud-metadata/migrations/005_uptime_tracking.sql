-- Migration: Track node uptime per epoch for payment calculation
-- This enables pro-rated payments based on actual online time

-- Track uptime per node per epoch for payment calculation
CREATE TABLE IF NOT EXISTS node_epoch_uptime (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    epoch BIGINT NOT NULL,
    epoch_start TIMESTAMP WITH TIME ZONE NOT NULL,
    epoch_end TIMESTAMP WITH TIME ZONE,

    -- Uptime tracking
    seconds_online BIGINT NOT NULL DEFAULT 0,
    seconds_offline BIGINT NOT NULL DEFAULT 0,
    last_status_change TIMESTAMP WITH TIME ZONE,

    -- Payment tracking
    payment_allocated BOOLEAN NOT NULL DEFAULT FALSE,
    payment_amount BIGINT,  -- In token smallest units (9 decimals for CYXWIZ)
    payment_tx_signature VARCHAR(128),  -- Solana transaction signature

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),

    UNIQUE(node_id, epoch)
);

-- Index for epoch queries (get all nodes for an epoch)
CREATE INDEX IF NOT EXISTS idx_node_epoch_uptime_epoch ON node_epoch_uptime(epoch);

-- Index for pending payments (nodes not yet paid)
CREATE INDEX IF NOT EXISTS idx_node_epoch_uptime_pending ON node_epoch_uptime(epoch)
    WHERE payment_allocated = FALSE;

-- Index for node payment history
CREATE INDEX IF NOT EXISTS idx_node_epoch_uptime_node ON node_epoch_uptime(node_id);

-- Track slashing events for audit and penalty enforcement
CREATE TABLE IF NOT EXISTS node_slashing_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    epoch BIGINT NOT NULL,
    reason VARCHAR(64) NOT NULL,  -- data_loss, extended_downtime, corrupted_data, failed_proofs
    slash_percent SMALLINT NOT NULL,  -- 5, 10, 15, or 50
    slash_amount BIGINT,  -- Amount slashed in tokens
    tx_signature VARCHAR(128),  -- Solana transaction signature
    details JSONB,  -- Additional context (chunk IDs, timestamps, etc.)
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_slashing_events_node ON node_slashing_events(node_id);
CREATE INDEX IF NOT EXISTS idx_slashing_events_epoch ON node_slashing_events(epoch);

-- Track epoch state for payment daemon
CREATE TABLE IF NOT EXISTS payment_epochs (
    epoch BIGINT PRIMARY KEY,
    started_at TIMESTAMP WITH TIME ZONE NOT NULL,
    ended_at TIMESTAMP WITH TIME ZONE,
    finalized BOOLEAN NOT NULL DEFAULT FALSE,
    total_pool_amount BIGINT,  -- Total tokens in pool for this epoch
    nodes_share BIGINT,  -- 85% of pool
    platform_share BIGINT,  -- 10% of pool
    community_share BIGINT,  -- 5% of pool
    nodes_paid INTEGER DEFAULT 0,  -- Count of nodes paid
    platform_claimed BOOLEAN DEFAULT FALSE,
    community_claimed BOOLEAN DEFAULT FALSE,
    finalize_tx_signature VARCHAR(128),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
);

-- View: Node uptime summary with payment status
CREATE OR REPLACE VIEW node_uptime_summary AS
SELECT
    n.id AS node_id,
    n.peer_id,
    n.wallet_address,
    n.storage_total,
    n.status,
    u.epoch,
    u.seconds_online,
    u.seconds_offline,
    u.seconds_online::float / NULLIF(u.seconds_online + u.seconds_offline, 0) AS uptime_ratio,
    u.payment_allocated,
    u.payment_amount
FROM nodes n
LEFT JOIN node_epoch_uptime u ON n.id = u.node_id;

COMMENT ON TABLE node_epoch_uptime IS 'Tracks node uptime per 7-day epoch for payment calculation';
COMMENT ON TABLE node_slashing_events IS 'Records slashing penalties for misbehaving nodes';
COMMENT ON TABLE payment_epochs IS 'Tracks epoch lifecycle and payment distribution state';
