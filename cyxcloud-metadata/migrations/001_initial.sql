-- CyxCloud Metadata Schema
-- Initial migration

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- NODES TABLE
-- Tracks registered storage nodes in the network
-- ============================================================================
CREATE TABLE nodes (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    peer_id VARCHAR(128) UNIQUE NOT NULL,  -- libp2p PeerId
    grpc_address VARCHAR(256) NOT NULL,     -- gRPC endpoint (ip:port)

    -- Capacity
    storage_total BIGINT NOT NULL DEFAULT 0,
    storage_used BIGINT NOT NULL DEFAULT 0,
    bandwidth_mbps INTEGER NOT NULL DEFAULT 0,
    max_connections INTEGER NOT NULL DEFAULT 100,

    -- Location
    datacenter VARCHAR(64),
    rack INTEGER DEFAULT 0,
    region VARCHAR(64),
    latitude DOUBLE PRECISION,
    longitude DOUBLE PRECISION,

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'offline',  -- online, offline, draining, maintenance
    last_heartbeat TIMESTAMP WITH TIME ZONE,
    failure_count INTEGER NOT NULL DEFAULT 0,

    -- Metadata
    version VARCHAR(32),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_nodes_status ON nodes(status);
CREATE INDEX idx_nodes_datacenter ON nodes(datacenter);
CREATE INDEX idx_nodes_region ON nodes(region);
CREATE INDEX idx_nodes_last_heartbeat ON nodes(last_heartbeat);

-- ============================================================================
-- FILES TABLE
-- Stores file metadata (user uploads)
-- ============================================================================
CREATE TABLE files (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),

    -- Identity
    name VARCHAR(1024) NOT NULL,
    path VARCHAR(4096) NOT NULL,           -- Full path including bucket/prefix
    content_hash BYTEA NOT NULL,           -- Blake3 hash of original content

    -- Size info
    size_bytes BIGINT NOT NULL,
    chunk_count INTEGER NOT NULL,

    -- Erasure coding info
    data_shards INTEGER NOT NULL,          -- Number of data shards
    parity_shards INTEGER NOT NULL,        -- Number of parity shards
    chunk_size INTEGER NOT NULL,           -- Size of each chunk before encoding

    -- Ownership
    owner_id UUID,                          -- User who uploaded
    bucket VARCHAR(256),                    -- S3-compatible bucket name

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'pending',  -- pending, uploading, complete, failed, deleted

    -- Metadata
    content_type VARCHAR(256),
    metadata JSONB,                         -- User-provided metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMP WITH TIME ZONE     -- Soft delete
);

CREATE INDEX idx_files_path ON files(path);
CREATE INDEX idx_files_bucket ON files(bucket);
CREATE INDEX idx_files_owner ON files(owner_id);
CREATE INDEX idx_files_status ON files(status);
CREATE INDEX idx_files_content_hash ON files(content_hash);
CREATE INDEX idx_files_created_at ON files(created_at);

-- ============================================================================
-- CHUNKS TABLE
-- Stores chunk metadata (mapping chunk_id to file)
-- ============================================================================
CREATE TABLE chunks (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    chunk_id BYTEA NOT NULL UNIQUE,        -- 32-byte Blake3 hash (ChunkId)
    file_id UUID NOT NULL REFERENCES files(id) ON DELETE CASCADE,

    -- Position in file
    shard_index INTEGER NOT NULL,          -- Which shard (0 to data+parity-1)
    is_parity BOOLEAN NOT NULL DEFAULT FALSE,

    -- Size
    size_bytes INTEGER NOT NULL,

    -- Replication
    replication_factor INTEGER NOT NULL DEFAULT 3,
    current_replicas INTEGER NOT NULL DEFAULT 0,

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'pending',  -- pending, stored, verified, corrupted

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    verified_at TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_chunks_file_id ON chunks(file_id);
CREATE INDEX idx_chunks_chunk_id ON chunks(chunk_id);
CREATE INDEX idx_chunks_status ON chunks(status);
CREATE INDEX idx_chunks_current_replicas ON chunks(current_replicas);

-- ============================================================================
-- CHUNK_LOCATIONS TABLE
-- Tracks which nodes store which chunks (many-to-many)
-- ============================================================================
CREATE TABLE chunk_locations (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    chunk_id BYTEA NOT NULL,               -- References chunks.chunk_id
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'pending',  -- pending, stored, verified, failed

    -- Verification
    last_verified TIMESTAMP WITH TIME ZONE,
    verification_failures INTEGER NOT NULL DEFAULT 0,

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),

    UNIQUE(chunk_id, node_id)
);

CREATE INDEX idx_chunk_locations_chunk_id ON chunk_locations(chunk_id);
CREATE INDEX idx_chunk_locations_node_id ON chunk_locations(node_id);
CREATE INDEX idx_chunk_locations_status ON chunk_locations(status);

-- ============================================================================
-- USERS TABLE
-- Basic user management for ownership
-- ============================================================================
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),

    -- Identity
    wallet_address VARCHAR(64) UNIQUE,     -- Solana wallet address
    email VARCHAR(256) UNIQUE,
    username VARCHAR(128) UNIQUE,

    -- Quotas
    storage_quota BIGINT NOT NULL DEFAULT 10737418240,  -- 10 GB default
    storage_used BIGINT NOT NULL DEFAULT 0,

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'active',  -- active, suspended, deleted

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_wallet ON users(wallet_address);
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_status ON users(status);

-- ============================================================================
-- BUCKETS TABLE
-- S3-compatible buckets
-- ============================================================================
CREATE TABLE buckets (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(256) NOT NULL UNIQUE,
    owner_id UUID NOT NULL REFERENCES users(id),

    -- Settings
    versioning_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    public_read BOOLEAN NOT NULL DEFAULT FALSE,

    -- Quotas
    max_size_bytes BIGINT,

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_buckets_owner ON buckets(owner_id);

-- ============================================================================
-- REPAIR_JOBS TABLE
-- Tracks chunk repair operations (for rebalancer)
-- ============================================================================
CREATE TABLE repair_jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    chunk_id BYTEA NOT NULL,

    -- Source and target
    source_node_id UUID REFERENCES nodes(id),
    target_node_id UUID NOT NULL REFERENCES nodes(id),

    -- Status
    status VARCHAR(32) NOT NULL DEFAULT 'pending',  -- pending, in_progress, completed, failed
    priority INTEGER NOT NULL DEFAULT 0,

    -- Progress
    started_at TIMESTAMP WITH TIME ZONE,
    completed_at TIMESTAMP WITH TIME ZONE,
    error_message TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,

    -- Metadata
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_repair_jobs_status ON repair_jobs(status);
CREATE INDEX idx_repair_jobs_priority ON repair_jobs(priority DESC);
CREATE INDEX idx_repair_jobs_chunk_id ON repair_jobs(chunk_id);

-- ============================================================================
-- FUNCTIONS
-- ============================================================================

-- Update timestamp trigger
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply to relevant tables
CREATE TRIGGER update_nodes_updated_at
    BEFORE UPDATE ON nodes
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER update_files_updated_at
    BEFORE UPDATE ON files
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER update_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

CREATE TRIGGER update_buckets_updated_at
    BEFORE UPDATE ON buckets
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

-- ============================================================================
-- VIEWS
-- ============================================================================

-- View: Chunk replication status
CREATE VIEW chunk_replication_status AS
SELECT
    c.chunk_id,
    c.file_id,
    c.replication_factor,
    c.current_replicas,
    c.replication_factor - c.current_replicas AS replicas_needed,
    CASE
        WHEN c.current_replicas >= c.replication_factor THEN 'healthy'
        WHEN c.current_replicas > 0 THEN 'under_replicated'
        ELSE 'missing'
    END AS health_status
FROM chunks c
WHERE c.status != 'pending';

-- View: Node storage summary
CREATE VIEW node_storage_summary AS
SELECT
    n.id,
    n.peer_id,
    n.grpc_address,
    n.storage_total,
    n.storage_used,
    n.storage_total - n.storage_used AS storage_available,
    CASE
        WHEN n.storage_total > 0
        THEN ROUND((n.storage_used::numeric / n.storage_total::numeric) * 100, 2)
        ELSE 0
    END AS utilization_percent,
    COUNT(cl.id) AS chunk_count,
    n.status,
    n.last_heartbeat
FROM nodes n
LEFT JOIN chunk_locations cl ON cl.node_id = n.id AND cl.status = 'stored'
GROUP BY n.id;
