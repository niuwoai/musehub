ALTER TABLE pull_requests ADD COLUMN base_oid CHAR(40);
ALTER TABLE pr_reviews ADD COLUMN approved_head_oid CHAR(40);

CREATE TABLE repo_required_checks (
    id UUID PRIMARY KEY,
    repo_id UUID NOT NULL REFERENCES repositories (id),
    check_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_repo_required_checks UNIQUE (repo_id, check_name)
);

CREATE TRIGGER repo_required_checks_set_updated_at
BEFORE UPDATE ON repo_required_checks
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_repo_required_checks_repo_id ON repo_required_checks (repo_id);

CREATE TABLE pr_check_runs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    pr_id UUID NOT NULL REFERENCES pull_requests (id),
    check_name TEXT NOT NULL,
    head_oid CHAR(40) NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'success', 'failure')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pr_check_runs UNIQUE (pr_id, check_name, head_oid)
);

CREATE INDEX idx_pr_check_runs_org_id ON pr_check_runs (org_id);
CREATE INDEX idx_pr_check_runs_repo_id ON pr_check_runs (repo_id);
CREATE INDEX idx_pr_check_runs_pr_id ON pr_check_runs (pr_id);

CREATE TRIGGER pr_check_runs_set_updated_at
BEFORE UPDATE ON pr_check_runs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
