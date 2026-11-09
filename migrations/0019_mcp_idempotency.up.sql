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
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT mcp_idempotency_state_check CHECK (state IN ('processing', 'completed', 'failed')),
    CONSTRAINT mcp_idempotency_key_len CHECK (char_length(idempotency_key) BETWEEN 8 AND 128),
    CONSTRAINT uk_mcp_idempotency UNIQUE (principal_id, tool_name, idempotency_key)
);

CREATE INDEX idx_mcp_idempotency_principal_id ON mcp_idempotency_records (principal_id);
CREATE INDEX idx_mcp_idempotency_org_id ON mcp_idempotency_records (org_id);
CREATE INDEX idx_mcp_idempotency_expires ON mcp_idempotency_records (expires_at);

CREATE TRIGGER mcp_idempotency_records_set_updated_at
BEFORE UPDATE ON mcp_idempotency_records
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
