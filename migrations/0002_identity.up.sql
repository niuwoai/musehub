CREATE TABLE principals (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('human', 'service', 'agent')),
    handle TEXT NOT NULL,
    handle_normalized TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_principals_handle_normalized UNIQUE (handle_normalized)
);

CREATE INDEX idx_principals_status ON principals (status);

CREATE TRIGGER principals_set_updated_at
BEFORE UPDATE ON principals
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE user_credentials (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    password_hash TEXT NOT NULL,
    password_changed_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_user_credentials_principal_id UNIQUE (principal_id)
);

CREATE INDEX idx_user_credentials_principal_id ON user_credentials (principal_id);

CREATE TRIGGER user_credentials_set_updated_at
BEFORE UPDATE ON user_credentials
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    token_hash BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_sessions_token_hash UNIQUE (token_hash)
);

CREATE INDEX idx_sessions_principal_id ON sessions (principal_id);
CREATE INDEX idx_sessions_expires_at ON sessions (expires_at);

CREATE TRIGGER sessions_set_updated_at
BEFORE UPDATE ON sessions
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE organizations (
    id UUID PRIMARY KEY,
    slug TEXT NOT NULL,
    slug_normalized TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_organizations_slug_normalized UNIQUE (slug_normalized)
);

CREATE TRIGGER organizations_set_updated_at
BEFORE UPDATE ON organizations
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE memberships (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    principal_id UUID NOT NULL REFERENCES principals (id),
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_memberships_org_id_principal_id UNIQUE (org_id, principal_id)
);

CREATE INDEX idx_memberships_org_id ON memberships (org_id);
CREATE INDEX idx_memberships_principal_id ON memberships (principal_id);

CREATE TRIGGER memberships_set_updated_at
BEFORE UPDATE ON memberships
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE access_tokens (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    org_id UUID REFERENCES organizations (id),
    name TEXT NOT NULL,
    token_prefix TEXT NOT NULL,
    token_hash BYTEA NOT NULL,
    scopes TEXT[] NOT NULL,
    scope_mode TEXT NOT NULL DEFAULT 'selected' CHECK (scope_mode IN ('selected', 'all_accessible')),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    bound_run_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_access_tokens_token_hash UNIQUE (token_hash)
);

CREATE INDEX idx_access_tokens_principal_id ON access_tokens (principal_id);
CREATE INDEX idx_access_tokens_org_id ON access_tokens (org_id);

CREATE TRIGGER access_tokens_set_updated_at
BEFORE UPDATE ON access_tokens
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE ssh_keys (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    name TEXT NOT NULL,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_ssh_keys_fingerprint UNIQUE (fingerprint)
);

CREATE INDEX idx_ssh_keys_principal_id ON ssh_keys (principal_id);

CREATE TRIGGER ssh_keys_set_updated_at
BEFORE UPDATE ON ssh_keys
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
