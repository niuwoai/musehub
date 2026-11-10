DROP INDEX IF EXISTS idx_pr_check_runs_attempt_id;
DROP INDEX IF EXISTS idx_pr_check_runs_reported_by;

ALTER TABLE pr_check_runs
    DROP COLUMN IF EXISTS expires_at,
    DROP COLUMN IF EXISTS attempt_id,
    DROP COLUMN IF EXISTS reported_by;
