-- MCP V1 contract: criteria, discussion, agent messages, job logs, deployment approvals.

ALTER TABLE implementation_plans
    ADD COLUMN IF NOT EXISTS steps JSONB NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE issue_acceptance_criteria (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    issue_revision_id UUID NOT NULL REFERENCES issue_revisions (id) ON DELETE CASCADE,
    ordinal INT NOT NULL CHECK (ordinal >= 1),
    description TEXT NOT NULL,
    verification_method TEXT NOT NULL DEFAULT 'manual',
    required BOOL NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_issue_criteria_revision_ord UNIQUE (issue_revision_id, ordinal)
);

CREATE INDEX idx_issue_criteria_org_id ON issue_acceptance_criteria (org_id);
CREATE INDEX idx_issue_criteria_repo ON issue_acceptance_criteria (repo_id);
CREATE INDEX idx_issue_criteria_revision ON issue_acceptance_criteria (issue_revision_id);

CREATE TRIGGER issue_acceptance_criteria_set_updated_at
BEFORE UPDATE ON issue_acceptance_criteria
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE discussion_comments (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    target_kind TEXT NOT NULL CHECK (target_kind IN ('issue', 'pr')),
    target_id UUID NOT NULL,
    author_id UUID NOT NULL REFERENCES principals (id),
    body TEXT NOT NULL,
    anchor JSONB,
    resolved BOOL NOT NULL DEFAULT false,
    outdated BOOL NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_discussion_comments_org_id ON discussion_comments (org_id);
CREATE INDEX idx_discussion_comments_repo_id_only ON discussion_comments (repo_id);
CREATE INDEX idx_discussion_comments_author_id ON discussion_comments (author_id);
CREATE INDEX idx_discussion_comments_target
    ON discussion_comments (repo_id, target_kind, target_id, created_at);

CREATE TRIGGER discussion_comments_set_updated_at
BEFORE UPDATE ON discussion_comments
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE agent_run_messages (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    agent_run_id UUID NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    sequence INT NOT NULL CHECK (sequence >= 1),
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_agent_run_messages_seq UNIQUE (agent_run_id, sequence)
);

CREATE INDEX idx_agent_run_messages_org_id ON agent_run_messages (org_id);
CREATE INDEX idx_agent_run_messages_repo_id ON agent_run_messages (repo_id);
CREATE INDEX idx_agent_run_messages_run ON agent_run_messages (agent_run_id);

CREATE TRIGGER agent_run_messages_set_updated_at
BEFORE UPDATE ON agent_run_messages
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

ALTER TABLE agent_runs
    DROP CONSTRAINT IF EXISTS agent_runs_state_check;

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_state_check CHECK (state IN (
        'queued',
        'running',
        'waiting_input',
        'succeeded',
        'failed',
        'cancel_requested',
        'canceled'
    ));

CREATE TABLE pipeline_job_logs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    job_id UUID NOT NULL REFERENCES pipeline_jobs (id) ON DELETE CASCADE,
    attempt_id UUID NOT NULL,
    chunk_index INT NOT NULL DEFAULT 0,
    text TEXT NOT NULL,
    complete BOOL NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pipeline_job_logs_chunk UNIQUE (job_id, attempt_id, chunk_index)
);

CREATE INDEX idx_pipeline_job_logs_org_id ON pipeline_job_logs (org_id);
CREATE INDEX idx_pipeline_job_logs_repo_id ON pipeline_job_logs (repo_id);
CREATE INDEX idx_pipeline_job_logs_job_id_only ON pipeline_job_logs (job_id);
CREATE INDEX idx_pipeline_job_logs_job ON pipeline_job_logs (job_id, attempt_id);

CREATE TRIGGER pipeline_job_logs_set_updated_at
BEFORE UPDATE ON pipeline_job_logs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE deployment_approvals (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    deployment_id UUID NOT NULL REFERENCES deployments (id) ON DELETE CASCADE,
    plan_hash CHAR(64) NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('approve', 'reject')),
    approver_id UUID NOT NULL REFERENCES principals (id),
    reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_deployment_approvals_org_id ON deployment_approvals (org_id);
CREATE INDEX idx_deployment_approvals_repo_id ON deployment_approvals (repo_id);
CREATE INDEX idx_deployment_approvals_approver_id ON deployment_approvals (approver_id);
CREATE INDEX idx_deployment_approvals_dep ON deployment_approvals (deployment_id);

CREATE TRIGGER deployment_approvals_set_updated_at
BEFORE UPDATE ON deployment_approvals
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
