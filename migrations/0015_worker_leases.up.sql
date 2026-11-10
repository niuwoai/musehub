-- Per-run worker leases: reconciliation only clears expired/unleased rows.
ALTER TABLE agent_runs
    ADD COLUMN lease_holder UUID,
    ADD COLUMN lease_until TIMESTAMPTZ;

ALTER TABLE pipeline_runs
    ADD COLUMN lease_holder UUID,
    ADD COLUMN lease_until TIMESTAMPTZ;

CREATE INDEX idx_agent_runs_active_lease ON agent_runs (lease_until)
    WHERE state IN ('queued', 'running', 'cancel_requested');

CREATE INDEX idx_pipeline_runs_active_lease ON pipeline_runs (lease_until)
    WHERE state IN ('queued', 'running', 'cancel_requested');
