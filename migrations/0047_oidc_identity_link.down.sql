DROP INDEX IF EXISTS idx_oidc_login_attempts_link_principal_id;

ALTER TABLE oidc_login_attempts
    DROP CONSTRAINT IF EXISTS chk_oidc_login_attempts_link_target,
    DROP COLUMN IF EXISTS link_session_token_hash,
    DROP COLUMN IF EXISTS link_principal_id;
