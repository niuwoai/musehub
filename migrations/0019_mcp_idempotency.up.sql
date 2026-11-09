CREATE TABLE mcp_idempotency_records (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    org_id UUID REFERENCES organizations (id),
    tool_name TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    state TEXT NOT NULL,
    response_json JSONB,
    resource_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT mcp_idempotency_state_check CHECK (state IN ('processing', 'completed', 'failed')),
    CONSTRAINT mcp_idempotency_key_len CHECK (char_length(idempotency_key) BETWEEN 8 AND 128),
    CONSTRAINT uk_mcp_idempotency UNIQUE (principal_id, tool_name, idempotency_key)
);

CREATE INDEX idx_mcp_idempotency_expires ON mcp_idempotency_records (expires_at);
