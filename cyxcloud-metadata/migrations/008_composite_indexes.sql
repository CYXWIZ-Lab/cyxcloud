-- ============================================================================
-- MIGRATION 008: Add composite indexes for query optimization
-- ============================================================================
-- These composite indexes optimize the most common query patterns:
-- 1. Finding chunks by file and status (rebalancer, health checks)
-- 2. Listing files in a bucket by path prefix
-- 3. Querying files by owner and status (quota checks, user dashboards)
-- ============================================================================

-- Composite index for chunk queries filtered by status
-- Used by: rebalancer scanning for under-replicated chunks per file
CREATE INDEX IF NOT EXISTS idx_chunks_file_status ON chunks(file_id, status);

-- Composite index for file listing within buckets (S3 ListObjects)
-- Used by: list_files_in_bucket with prefix filtering
CREATE INDEX IF NOT EXISTS idx_files_bucket_path ON files(bucket, path);

-- Composite index for user file queries filtered by status
-- Used by: quota calculation, user dashboard file listings
CREATE INDEX IF NOT EXISTS idx_files_owner_status ON files(owner_id, status);
