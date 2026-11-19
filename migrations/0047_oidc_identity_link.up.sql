ALTER TABLE oidc_login_attempts
    ADD COLUMN link_principal_id UUID REFERENCES principals (id) ON DELETE CASCADE,
    ADD COLUMN link_session_token_hash BYTEA,
    ADD CONSTRAINT chk_oidc_login_attempts_link_target
        CHECK ((link_principal_id IS NULL) = (link_session_token_hash IS NULL));

CREATE INDEX idx_oidc_login_attempts_link_principal_id
    ON oidc_login_attempts (link_principal_id);
