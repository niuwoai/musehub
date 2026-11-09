-- Fencing token: renew/worker must match current epoch (stale workers cannot extend lease).
ALTER TABLE agent_runs ADD COLUMN lease_epoch BIGINT NOT NULL DEFAULT 0;
ALTER TABLE pipeline_runs ADD COLUMN lease_epoch BIGINT NOT NULL DEFAULT 0;
