-- ============================================================================
-- MIGRATION 006: Add chunk_index to chunks table
-- ============================================================================
-- For multi-chunk files, we need to know which original chunk each shard
-- belongs to. This enables proper erasure decoding during retrieval.
-- ============================================================================

-- Add chunk_index column (0-based index of the original chunk within the file)
ALTER TABLE chunks ADD COLUMN chunk_index INTEGER NOT NULL DEFAULT 0;

-- Create index for efficient queries grouping by chunk
CREATE INDEX idx_chunks_file_chunk ON chunks(file_id, chunk_index);
