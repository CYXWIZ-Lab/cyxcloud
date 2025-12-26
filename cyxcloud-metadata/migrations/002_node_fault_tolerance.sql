-- Migration: Node Fault Tolerance
-- Adds columns for tracking node offline duration and status transitions
--
-- New columns:
--   first_offline_at: Records when node first went offline (for drain/remove thresholds)
--   status_changed_at: Records when status last changed (for recovery quarantine)
--
-- New status value: 'recovering' (quarantine period after reconnect)

-- Add column to track when node first went offline
-- This is used to calculate drain threshold (4 hours) and remove threshold (7 days)
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS first_offline_at TIMESTAMP WITH TIME ZONE;

-- Add column to track when status last changed
-- This is used for recovery quarantine period (5 minutes)
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS status_changed_at TIMESTAMP WITH TIME ZONE DEFAULT NOW();

-- Create indexes for efficient queries
-- These support the periodic node monitor scans
CREATE INDEX IF NOT EXISTS idx_nodes_first_offline_at ON nodes(first_offline_at);
CREATE INDEX IF NOT EXISTS idx_nodes_status_changed_at ON nodes(status_changed_at);

-- Update existing nodes to have status_changed_at set
UPDATE nodes SET status_changed_at = COALESCE(last_heartbeat, created_at, NOW())
WHERE status_changed_at IS NULL;

-- Add comment documenting the valid status values
COMMENT ON COLUMN nodes.status IS 'Node status: online, offline, recovering, draining, maintenance';
COMMENT ON COLUMN nodes.first_offline_at IS 'Timestamp when node first went offline (reset on recovery)';
COMMENT ON COLUMN nodes.status_changed_at IS 'Timestamp of last status transition';
