DROP INDEX IF EXISTS idx_agent_runs_pr_id;

ALTER TABLE agent_runs
    DROP COLUMN IF EXISTS fix_round,
    DROP COLUMN IF EXISTS head_oid,
    DROP COLUMN IF EXISTS pr_id;
