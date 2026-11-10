DROP TRIGGER IF EXISTS token_repository_grants_set_updated_at ON token_repository_grants;
DROP INDEX IF EXISTS idx_token_repository_grants_org_id;
ALTER TABLE token_repository_grants DROP COLUMN IF EXISTS updated_at;
