DROP INDEX IF EXISTS idx_deployments_rollback_of_id;
ALTER TABLE deployments DROP COLUMN IF EXISTS rollback_of_id;
DROP INDEX IF EXISTS idx_audit_events_resource;
DROP INDEX IF EXISTS idx_audit_events_org_time;
DROP TABLE IF EXISTS audit_events;
