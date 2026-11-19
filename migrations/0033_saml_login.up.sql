CREATE TABLE saml_login_attempts (
    relay_hash BYTEA PRIMARY KEY,
    binding_hash BYTEA NOT NULL,
    request_id TEXT NOT NULL UNIQUE,
    idp_entity_id TEXT NOT NULL,
    expected_binding TEXT NOT NULL,
    acs_url TEXT NOT NULL,
    acs_binding TEXT NOT NULL,
    return_path TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER saml_login_attempts_set_updated_at
BEFORE UPDATE ON saml_login_attempts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_saml_login_attempts_expires_at ON saml_login_attempts (expires_at);

CREATE TABLE saml_replay_entries (
    cache_key_hash BYTEA PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER saml_replay_entries_set_updated_at
BEFORE UPDATE ON saml_replay_entries
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_saml_replay_entries_expires_at ON saml_replay_entries (expires_at);
