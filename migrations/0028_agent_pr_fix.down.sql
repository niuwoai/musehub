DROP INDEX IF EXISTS uk_agent_runs_pr_inflight;
DROP INDEX IF EXISTS uk_agent_runs_pr_fix_round;
DROP INDEX IF EXISTS idx_agent_runs_pr_id;
DROP INDEX IF EXISTS idx_agent_runs_pr_id_leading;

ALTER TABLE agent_runs
    DROP COLUMN IF EXISTS fix_round,
    DROP COLUMN IF EXISTS head_oid,
    DROP COLUMN IF EXISTS pr_id;
