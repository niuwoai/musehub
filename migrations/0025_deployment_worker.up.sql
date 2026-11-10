-- Remote deployment worker: queued launches + optional lease fencing.
ALTER TABLE deployments
    ADD COLUMN IF NOT EXISTS lease_holder UUID,
    ADD COLUMN IF NOT EXISTS lease_until TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS lease_epoch BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS cancel_requested BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS launch_payload JSONB;

CREATE INDEX idx_deployments_queued ON deployments (org_id, state, created_at)
    WHERE state = 'queued';
