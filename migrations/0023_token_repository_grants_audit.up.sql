ALTER TABLE token_repository_grants
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP;

CREATE INDEX IF NOT EXISTS idx_token_repository_grants_org_id ON token_repository_grants (org_id);

DROP TRIGGER IF EXISTS token_repository_grants_set_updated_at ON token_repository_grants;
CREATE TRIGGER token_repository_grants_set_updated_at
BEFORE UPDATE ON token_repository_grants
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
