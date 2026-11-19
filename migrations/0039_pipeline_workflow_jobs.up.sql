CREATE TABLE pipeline_workflow_jobs (
    id UUID PRIMARY KEY,
    pipeline_run_id UUID NOT NULL REFERENCES pipeline_runs (id) ON DELETE CASCADE,
    job_key TEXT NOT NULL,
    image_ref TEXT NOT NULL,
    command JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'canceled', 'skipped')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    error_code TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pipeline_workflow_jobs_run_key UNIQUE (pipeline_run_id, job_key)
);

CREATE INDEX idx_pipeline_workflow_jobs_run_state
    ON pipeline_workflow_jobs (pipeline_run_id, state, job_key);

CREATE TRIGGER pipeline_workflow_jobs_set_updated_at
BEFORE UPDATE ON pipeline_workflow_jobs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE pipeline_workflow_job_dependencies (
    job_id UUID NOT NULL REFERENCES pipeline_workflow_jobs (id) ON DELETE CASCADE,
    needs_job_id UUID NOT NULL REFERENCES pipeline_workflow_jobs (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pk_pipeline_workflow_job_dependencies PRIMARY KEY (job_id, needs_job_id),
    CONSTRAINT ck_pipeline_workflow_job_dependencies_not_self CHECK (job_id <> needs_job_id)
);

CREATE INDEX idx_pipeline_workflow_job_dependencies_needs
    ON pipeline_workflow_job_dependencies (needs_job_id);

CREATE TABLE pipeline_workflow_job_attempts (
    id UUID PRIMARY KEY,
    workflow_job_id UUID NOT NULL REFERENCES pipeline_workflow_jobs (id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    state TEXT NOT NULL
        CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'canceled', 'lost')),
    error_code TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pipeline_workflow_job_attempts_number UNIQUE (workflow_job_id, attempt_number)
);

CREATE INDEX idx_pipeline_workflow_job_attempts_job_created
    ON pipeline_workflow_job_attempts (workflow_job_id, created_at, id);

CREATE TRIGGER pipeline_workflow_job_attempts_set_updated_at
BEFORE UPDATE ON pipeline_workflow_job_attempts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
