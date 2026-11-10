CREATE TABLE token_repository_grants (
    id UUID PRIMARY KEY,
    token_id UUID NOT NULL REFERENCES access_tokens (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_token_repository_grants UNIQUE (token_id, repo_id)
);

CREATE INDEX idx_token_repository_grants_token_id ON token_repository_grants (token_id);
CREATE INDEX idx_token_repository_grants_repo_id ON token_repository_grants (repo_id);
CREATE INDEX idx_token_repository_grants_org_id ON token_repository_grants (org_id);

CREATE TRIGGER token_repository_grants_set_updated_at
BEFORE UPDATE ON token_repository_grants
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
