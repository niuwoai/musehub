CREATE TABLE environments (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_environments_repo_slug UNIQUE (repo_id, slug)
);

CREATE INDEX idx_environments_org_id ON environments (org_id);
CREATE INDEX idx_environments_repo_id ON environments (repo_id);

CREATE TRIGGER environments_set_updated_at
BEFORE UPDATE ON environments
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE pipeline_runs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    source_oid CHAR(40) NOT NULL,
    workflow_name TEXT NOT NULL,
    state TEXT NOT NULL,
    requested_by UUID NOT NULL REFERENCES principals (id),
    run_attempt_id UUID NOT NULL,
    pr_id UUID REFERENCES pull_requests (id),
    issue_id UUID REFERENCES issues (id),
    agent_run_id UUID REFERENCES agent_runs (id),
    cancel_requested BOOLEAN NOT NULL DEFAULT FALSE,
    error_code TEXT,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_pipeline_runs_org_id ON pipeline_runs (org_id);
CREATE INDEX idx_pipeline_runs_repo_id ON pipeline_runs (repo_id);
CREATE INDEX idx_pipeline_runs_source_oid ON pipeline_runs (repo_id, source_oid);
CREATE INDEX idx_pipeline_runs_pr_id ON pipeline_runs (pr_id);
CREATE INDEX idx_pipeline_runs_issue_id ON pipeline_runs (issue_id);
CREATE INDEX idx_pipeline_runs_agent_run_id ON pipeline_runs (agent_run_id);
CREATE INDEX idx_pipeline_runs_requested_by ON pipeline_runs (requested_by);

CREATE TRIGGER pipeline_runs_set_updated_at
BEFORE UPDATE ON pipeline_runs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE pipeline_jobs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    pipeline_run_id UUID NOT NULL REFERENCES pipeline_runs (id),
    job_key TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pipeline_jobs_run_key UNIQUE (pipeline_run_id, job_key)
);

CREATE INDEX idx_pipeline_jobs_run_id ON pipeline_jobs (pipeline_run_id);
CREATE INDEX idx_pipeline_jobs_org_id ON pipeline_jobs (org_id);
CREATE INDEX idx_pipeline_jobs_repo_id ON pipeline_jobs (repo_id);

CREATE TRIGGER pipeline_jobs_set_updated_at
BEFORE UPDATE ON pipeline_jobs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE artifacts (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    pipeline_run_id UUID NOT NULL REFERENCES pipeline_runs (id),
    name TEXT NOT NULL,
    sha256 CHAR(64) NOT NULL,
    byte_length BIGINT NOT NULL CHECK (byte_length >= 0),
    storage_path TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('uploading', 'ready', 'quarantined')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_artifacts_run_name UNIQUE (pipeline_run_id, name)
);

CREATE INDEX idx_artifacts_org_id ON artifacts (org_id);
CREATE INDEX idx_artifacts_repo_id ON artifacts (repo_id);
CREATE INDEX idx_artifacts_repo_sha ON artifacts (repo_id, sha256);

CREATE TRIGGER artifacts_set_updated_at
BEFORE UPDATE ON artifacts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE releases (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    version TEXT NOT NULL,
    tag_name TEXT NOT NULL,
    commit_oid CHAR(40) NOT NULL,
    pipeline_run_id UUID NOT NULL REFERENCES pipeline_runs (id),
    manifest_sha256 CHAR(64),
    state TEXT NOT NULL CHECK (state IN ('draft', 'published')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_releases_repo_version UNIQUE (repo_id, version)
);

CREATE INDEX idx_releases_org_id ON releases (org_id);
CREATE INDEX idx_releases_repo_id ON releases (repo_id);
CREATE INDEX idx_releases_pipeline_run_id ON releases (pipeline_run_id);

CREATE TRIGGER releases_set_updated_at
BEFORE UPDATE ON releases
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE release_pr_links (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    release_id UUID NOT NULL REFERENCES releases (id),
    pr_id UUID NOT NULL REFERENCES pull_requests (id),
    merged_commit_oid CHAR(40) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_release_pr_links UNIQUE (release_id, pr_id)
);

CREATE INDEX idx_release_pr_links_org_id ON release_pr_links (org_id);
CREATE INDEX idx_release_pr_links_repo_id ON release_pr_links (repo_id);
CREATE INDEX idx_release_pr_links_release_id ON release_pr_links (release_id);
CREATE INDEX idx_release_pr_links_pr_id ON release_pr_links (pr_id);

CREATE TRIGGER release_pr_links_set_updated_at
BEFORE UPDATE ON release_pr_links
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE deployments (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    environment_id UUID NOT NULL REFERENCES environments (id),
    release_id UUID NOT NULL REFERENCES releases (id),
    adapter_kind TEXT NOT NULL,
    state TEXT NOT NULL,
    health_summary JSONB,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_deployments_env_id ON deployments (environment_id);
CREATE INDEX idx_deployments_org_id ON deployments (org_id);
CREATE INDEX idx_deployments_repo_id ON deployments (repo_id);

CREATE TRIGGER deployments_set_updated_at
BEFORE UPDATE ON deployments
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
