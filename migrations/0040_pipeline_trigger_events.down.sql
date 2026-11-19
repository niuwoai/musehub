DROP INDEX IF EXISTS uk_pipeline_runs_trigger_key;

ALTER TABLE pipeline_runs
    DROP COLUMN IF EXISTS trigger_key,
    DROP COLUMN IF EXISTS trigger_ref,
    DROP COLUMN IF EXISTS trigger_event,
    DROP CONSTRAINT IF EXISTS pipeline_runs_run_kind_check;

ALTER TABLE pipeline_runs
    ADD CONSTRAINT pipeline_runs_run_kind_check
        CHECK (run_kind IN ('pr_validation', 'release_build'));
