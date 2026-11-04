-- Files are erasure-coded: each shard is stored once and a lost shard is
-- rebuilt from the parity shards on read, so a shard's target is one copy.
-- Rows written with a target of 3 made every shard look under-replicated,
-- and the rebalancer planned repairs it could never run on every scan
-- (TOFIX171). REBALANCER_REPLICATION_FACTOR still asks for more.
ALTER TABLE chunks ALTER COLUMN replication_factor SET DEFAULT 1;
UPDATE chunks SET replication_factor = 1 WHERE replication_factor <> 1;
