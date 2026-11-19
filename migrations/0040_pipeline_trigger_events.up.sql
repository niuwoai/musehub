ALTER TABLE pipeline_runs
    DROP CONSTRAINT pipeline_runs_run_kind_check;

ALTER TABLE pipeline_runs
    ADD CONSTRAINT pipeline_runs_run_kind_check
        CHECK (run_kind IN ('pr_validation', 'release_build', 'workflow')),
    ADD COLUMN trigger_event TEXT
        CHECK (trigger_event IS NULL OR trigger_event IN ('pull_request', 'push', 'manual', 'tag')),
    ADD COLUMN trigger_ref TEXT,
    ADD COLUMN trigger_key TEXT;

CREATE UNIQUE INDEX uk_pipeline_runs_trigger_key
    ON pipeline_runs (repo_id, trigger_key)
    WHERE trigger_key IS NOT NULL;
