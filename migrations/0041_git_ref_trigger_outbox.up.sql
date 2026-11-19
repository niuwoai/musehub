CREATE TABLE git_ref_trigger_outbox (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    repo_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    storage_key UUID NOT NULL,
    principal_id UUID NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
    git_ref TEXT NOT NULL,
    source_oid TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'processing', 'completed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMPTZ,
    UNIQUE (repo_id, git_ref, source_oid)
);

CREATE INDEX idx_git_ref_trigger_outbox_org_id
    ON git_ref_trigger_outbox (org_id);

CREATE INDEX idx_git_ref_trigger_outbox_principal_id
    ON git_ref_trigger_outbox (principal_id);

CREATE INDEX idx_git_ref_trigger_outbox_pending
    ON git_ref_trigger_outbox (created_at, id)
    WHERE state = 'pending';

CREATE INDEX idx_git_ref_trigger_outbox_processing
    ON git_ref_trigger_outbox (updated_at, id)
    WHERE state = 'processing';
