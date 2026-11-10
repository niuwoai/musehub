DROP INDEX IF EXISTS idx_pipeline_runs_release_active;
DROP INDEX IF EXISTS idx_pipeline_runs_pr_validation;
ALTER TABLE pipeline_runs DROP COLUMN IF EXISTS run_kind;
