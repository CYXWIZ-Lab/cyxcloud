-- Migration: the secret a node hands the gateway at registration; the gateway
-- presents it (authorization: Bearer <secret>) on every chunk RPC to that node,
-- so a node accepts shard traffic from its gateway only (TOFIX176 P2).
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS access_secret VARCHAR(128);

COMMENT ON COLUMN nodes.access_secret IS 'Per-node secret the gateway presents on chunk RPCs to this node';
