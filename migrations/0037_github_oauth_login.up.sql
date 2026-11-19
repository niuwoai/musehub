CREATE TABLE github_login_attempts (
    state_hash BYTEA PRIMARY KEY,
    binding_hash BYTEA NOT NULL,
    return_path TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER github_login_attempts_set_updated_at
BEFORE UPDATE ON github_login_attempts
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE INDEX idx_github_login_attempts_expires_at ON github_login_attempts (expires_at);
