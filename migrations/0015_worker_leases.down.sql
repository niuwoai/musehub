DROP INDEX IF EXISTS idx_pipeline_runs_active_lease;
DROP INDEX IF EXISTS idx_agent_runs_active_lease;
ALTER TABLE pipeline_runs DROP COLUMN IF EXISTS lease_until;
ALTER TABLE pipeline_runs DROP COLUMN IF EXISTS lease_holder;
ALTER TABLE agent_runs DROP COLUMN IF EXISTS lease_until;
ALTER TABLE agent_runs DROP COLUMN IF EXISTS lease_holder;
