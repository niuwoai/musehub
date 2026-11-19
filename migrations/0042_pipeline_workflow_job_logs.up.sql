CREATE TABLE pipeline_workflow_job_logs (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    repo_id UUID NOT NULL REFERENCES repositories (id) ON DELETE CASCADE,
    workflow_job_id UUID NOT NULL REFERENCES pipeline_workflow_jobs (id) ON DELETE CASCADE,
    attempt_id UUID NOT NULL,
    chunk_index INT NOT NULL DEFAULT 0,
    text TEXT NOT NULL,
    complete BOOL NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pipeline_workflow_job_logs_chunk UNIQUE (workflow_job_id, attempt_id, chunk_index)
);

CREATE INDEX idx_pipeline_workflow_job_logs_org_id ON pipeline_workflow_job_logs (org_id);
CREATE INDEX idx_pipeline_workflow_job_logs_repo_id ON pipeline_workflow_job_logs (repo_id);
CREATE INDEX idx_pipeline_workflow_job_logs_job ON pipeline_workflow_job_logs (workflow_job_id, attempt_id);

CREATE TRIGGER pipeline_workflow_job_logs_set_updated_at
BEFORE UPDATE ON pipeline_workflow_job_logs
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
