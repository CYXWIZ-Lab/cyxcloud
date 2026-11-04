-- A removed node must not be held back by repair jobs that name it: the
-- node monitor's auto-removal (offline > 7 days) failed on these foreign
-- keys. A job whose source is gone keeps its row with no source; a job
-- whose target is gone has nothing left to do.
ALTER TABLE repair_jobs
    DROP CONSTRAINT repair_jobs_source_node_id_fkey,
    ADD CONSTRAINT repair_jobs_source_node_id_fkey
        FOREIGN KEY (source_node_id) REFERENCES nodes(id) ON DELETE SET NULL;

ALTER TABLE repair_jobs
    DROP CONSTRAINT repair_jobs_target_node_id_fkey,
    ADD CONSTRAINT repair_jobs_target_node_id_fkey
        FOREIGN KEY (target_node_id) REFERENCES nodes(id) ON DELETE CASCADE;
