-- Protected environments: approval gate before deploy execution.

ALTER TABLE environments
    ADD COLUMN IF NOT EXISTS requires_approval BOOL NOT NULL DEFAULT false;

ALTER TABLE environments
    ADD COLUMN IF NOT EXISTS row_version BIGINT NOT NULL DEFAULT 1;

ALTER TABLE deployments
    ADD COLUMN IF NOT EXISTS plan_hash CHAR(64);

ALTER TABLE deployments
    ADD COLUMN IF NOT EXISTS requested_by UUID REFERENCES principals (id);

ALTER TABLE deployments
    ADD COLUMN IF NOT EXISTS approved_by UUID REFERENCES principals (id);

CREATE INDEX idx_deployments_requested_by ON deployments (requested_by);
CREATE INDEX idx_deployments_approved_by ON deployments (approved_by);
CREATE INDEX idx_environments_requires_approval ON environments (repo_id, requires_approval);
