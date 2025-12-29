-- Migration: Add wallet and public key columns to nodes table
-- This enables node operators to receive payments and verify identity

-- Add wallet address column (Solana address, max 44 chars base58)
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS wallet_address VARCHAR(64);

-- Add public key column (Ed25519 public key, hex encoded)
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS public_key VARCHAR(128);

-- Index for wallet lookups (for payment distribution queries)
CREATE INDEX IF NOT EXISTS idx_nodes_wallet ON nodes(wallet_address) WHERE wallet_address IS NOT NULL;

-- Comment for documentation
COMMENT ON COLUMN nodes.wallet_address IS 'Solana wallet address for receiving storage payments';
COMMENT ON COLUMN nodes.public_key IS 'Ed25519 public key for node authentication';
