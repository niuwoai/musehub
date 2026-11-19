CREATE TABLE scim_tokens (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    created_by UUID NOT NULL REFERENCES principals (id),
    token_prefix TEXT NOT NULL,
    token_hash BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_scim_tokens_token_hash UNIQUE (token_hash)
);

CREATE INDEX idx_scim_tokens_org_id ON scim_tokens (org_id);
CREATE INDEX idx_scim_tokens_created_by ON scim_tokens (created_by);
CREATE TRIGGER scim_tokens_set_updated_at
BEFORE UPDATE ON scim_tokens
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE scim_identities (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    principal_id UUID NOT NULL REFERENCES principals (id),
    external_id TEXT NOT NULL CHECK (length(external_id) > 0),
    display_name TEXT NOT NULL CHECK (length(display_name) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_scim_identities_org_external_id UNIQUE (org_id, external_id),
    CONSTRAINT uk_scim_identities_org_principal UNIQUE (org_id, principal_id)
);

CREATE INDEX idx_scim_identities_principal_id ON scim_identities (principal_id);
CREATE TRIGGER scim_identities_set_updated_at
BEFORE UPDATE ON scim_identities
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
