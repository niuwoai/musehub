CREATE TABLE pipeline_workflow_snapshots (
    pipeline_run_id UUID PRIMARY KEY REFERENCES pipeline_runs(id) ON DELETE CASCADE,
    source_oid TEXT NOT NULL,
    workflow_path TEXT NOT NULL,
    content_hash CHAR(64) NOT NULL,
    definition JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT pipeline_workflow_snapshots_hash_check
        CHECK (content_hash ~ '^[0-9a-f]{64}$')
);
