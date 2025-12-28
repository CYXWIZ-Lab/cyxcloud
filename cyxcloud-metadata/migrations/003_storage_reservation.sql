-- CyxCloud Storage Reservation Migration
-- Adds storage_reserved column for tracking gateway-reserved storage per node

-- Add storage_reserved column to nodes table
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS storage_reserved BIGINT NOT NULL DEFAULT 0;

-- Create index for efficient queries on storage
CREATE INDEX IF NOT EXISTS idx_nodes_storage_reserved ON nodes(storage_reserved);

-- Update the node_storage_summary view to account for reserved storage
DROP VIEW IF EXISTS node_storage_summary;

CREATE VIEW node_storage_summary AS
SELECT
    n.id,
    n.peer_id,
    n.grpc_address,
    n.storage_total,
    n.storage_reserved,
    n.storage_used,
    -- Available = Total - Reserved - Used
    GREATEST(0, n.storage_total - n.storage_reserved - n.storage_used) AS storage_available,
    -- User-allocatable capacity (excluding reserved)
    GREATEST(0, n.storage_total - n.storage_reserved) AS storage_allocatable,
    CASE
        WHEN (n.storage_total - n.storage_reserved) > 0
        THEN ROUND((n.storage_used::numeric / (n.storage_total - n.storage_reserved)::numeric) * 100, 2)
        ELSE 0
    END AS utilization_percent,
    COUNT(cl.id) AS chunk_count,
    n.status,
    n.last_heartbeat
FROM nodes n
LEFT JOIN chunk_locations cl ON cl.node_id = n.id AND cl.status = 'stored'
GROUP BY n.id;

-- Add comment for documentation
COMMENT ON COLUMN nodes.storage_reserved IS 'Gateway-reserved storage in bytes (2GB default for system use: metadata, parity buffer, rebalancing, health checks)';
