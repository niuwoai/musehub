CREATE TABLE issues (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    number BIGINT NOT NULL,
    author_id UUID NOT NULL REFERENCES principals (id),
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL CHECK (state IN ('open', 'closed', 'in_progress')) DEFAULT 'open',
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_issues_repo_number UNIQUE (repo_id, number)
);

CREATE INDEX idx_issues_org_id ON issues (org_id);
CREATE INDEX idx_issues_repo_id_state ON issues (repo_id, state, created_at);

CREATE TRIGGER issues_set_updated_at
BEFORE UPDATE ON issues
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE agent_runs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    issue_id UUID NOT NULL REFERENCES issues (id),
    requested_by UUID NOT NULL REFERENCES principals (id),
    adapter_kind TEXT NOT NULL,
    work_branch TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'running', 'succeeded', 'failed',
        'cancel_requested', 'canceled'
    )) DEFAULT 'queued',
    budget_microusd_limit BIGINT,
    budget_tokens_limit BIGINT,
    usage_microusd BIGINT NOT NULL DEFAULT 0,
    usage_tokens BIGINT NOT NULL DEFAULT 0,
    result_commit_oid CHAR(40),
    error_code TEXT,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_agent_runs_repo_state ON agent_runs (repo_id, state, created_at);
CREATE INDEX idx_agent_runs_issue_id ON agent_runs (issue_id);

CREATE TRIGGER agent_runs_set_updated_at
BEFORE UPDATE ON agent_runs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
