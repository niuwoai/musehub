DROP INDEX IF EXISTS idx_environments_requires_approval;
DROP INDEX IF EXISTS idx_deployments_approved_by;
DROP INDEX IF EXISTS idx_deployments_requested_by;
ALTER TABLE deployments DROP COLUMN IF EXISTS approved_by;
ALTER TABLE deployments DROP COLUMN IF EXISTS requested_by;
ALTER TABLE deployments DROP COLUMN IF EXISTS plan_hash;
ALTER TABLE environments DROP COLUMN IF EXISTS row_version;
ALTER TABLE environments DROP COLUMN IF EXISTS requires_approval;
