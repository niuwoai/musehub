CREATE TABLE external_identities (
    id UUID PRIMARY KEY,
    principal_id UUID NOT NULL REFERENCES principals (id),
    issuer TEXT NOT NULL CHECK (length(issuer) > 0),
    subject TEXT NOT NULL CHECK (length(subject) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_external_identities_issuer_subject UNIQUE (issuer, subject)
);

CREATE INDEX idx_external_identities_principal_id ON external_identities (principal_id);
CREATE TRIGGER external_identities_set_updated_at
BEFORE UPDATE ON external_identities
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE oidc_login_attempts (
    state_hash BYTEA PRIMARY KEY,
    binding_hash BYTEA NOT NULL,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    return_path TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER oidc_login_attempts_set_updated_at
BEFORE UPDATE ON oidc_login_attempts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_oidc_login_attempts_expires_at ON oidc_login_attempts (expires_at);
