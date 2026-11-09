ALTER TABLE pipeline_runs
    ADD COLUMN run_kind TEXT NOT NULL DEFAULT 'release_build'
    CHECK (run_kind IN ('pr_validation', 'release_build'));

CREATE INDEX idx_pipeline_runs_pr_validation ON pipeline_runs (repo_id, pr_id, run_kind, source_oid)
    WHERE run_kind = 'pr_validation';

CREATE INDEX idx_pipeline_runs_release_active ON pipeline_runs (repo_id, source_oid, run_kind)
    WHERE run_kind = 'release_build';
