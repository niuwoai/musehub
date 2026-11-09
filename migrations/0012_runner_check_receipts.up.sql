CREATE TABLE runner_check_receipts (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    pipeline_run_id UUID NOT NULL REFERENCES pipeline_runs (id),
    attempt_id UUID NOT NULL,
    pr_id UUID NOT NULL REFERENCES pull_requests (id),
    check_name TEXT NOT NULL,
    head_oid CHAR(40) NOT NULL,
    reported_by UUID NOT NULL REFERENCES principals (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_runner_check_receipt_attempt UNIQUE (attempt_id)
);

CREATE INDEX idx_runner_check_receipts_pipeline ON runner_check_receipts (pipeline_run_id);
CREATE INDEX idx_runner_check_receipts_repo ON runner_check_receipts (repo_id);
CREATE INDEX idx_runner_check_receipts_org_id ON runner_check_receipts (org_id);
CREATE INDEX idx_runner_check_receipts_pr_id ON runner_check_receipts (pr_id);
CREATE INDEX idx_runner_check_receipts_reported_by ON runner_check_receipts (reported_by);

CREATE TRIGGER runner_check_receipts_set_updated_at
BEFORE UPDATE ON runner_check_receipts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_pipeline_jobs_org_repo ON pipeline_jobs (org_id, repo_id);
CREATE INDEX idx_artifacts_pipeline_run ON artifacts (pipeline_run_id);
CREATE INDEX idx_deployments_release_id ON deployments (release_id);
