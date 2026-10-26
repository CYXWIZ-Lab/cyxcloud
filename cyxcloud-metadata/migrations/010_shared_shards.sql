-- Migration: two files may hold identical shards.
--
-- Shard ids are content hashes (BLAKE3 of the shard bytes), so uploading the
-- same content twice (the same Parquet file in two datasets) produces the same
-- shard ids. chunks.chunk_id was UNIQUE, so every shard row of the second file
-- was refused and the file could not be read back, while the upload reported
-- success (found 2026-10-26, TOFIX171). A shard row is unique per position in
-- its file instead; the shard id stays indexed for lookups. chunk_locations
-- stays keyed by (chunk_id, node_id): one stored piece on a node serves every
-- file that contains it. File deletes are soft, so a shared piece is never
-- removed from under another file (shard garbage collection must count
-- references first).
ALTER TABLE chunks DROP CONSTRAINT IF EXISTS chunks_chunk_id_key;
CREATE INDEX IF NOT EXISTS idx_chunks_chunk_id ON chunks(chunk_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_file_position
    ON chunks(file_id, chunk_index, shard_index);
