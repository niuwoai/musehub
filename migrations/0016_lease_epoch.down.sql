ALTER TABLE pipeline_runs DROP COLUMN IF EXISTS lease_epoch;
ALTER TABLE agent_runs DROP COLUMN IF EXISTS lease_epoch;
