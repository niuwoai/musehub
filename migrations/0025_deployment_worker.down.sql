DROP INDEX IF EXISTS idx_deployments_queued;
ALTER TABLE deployments
    DROP COLUMN IF EXISTS launch_payload,
    DROP COLUMN IF EXISTS cancel_requested,
    DROP COLUMN IF EXISTS lease_epoch,
    DROP COLUMN IF EXISTS lease_until,
    DROP COLUMN IF EXISTS lease_holder;
