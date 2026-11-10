CREATE TABLE audit_events (
    id UUID PRIMARY KEY,
    org_id UUID REFERENCES organizations (id),
    repo_id UUID REFERENCES repositories (id),
    actor_id UUID NOT NULL REFERENCES principals (id),
    initiator_id UUID REFERENCES principals (id),
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id UUID NOT NULL,
    request_id UUID,
    decision TEXT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_audit_events_org_id ON audit_events (org_id);
CREATE INDEX idx_audit_events_org_time ON audit_events (org_id, occurred_at, id);
CREATE INDEX idx_audit_events_resource ON audit_events (resource_type, resource_id, occurred_at, id);
CREATE INDEX idx_audit_events_actor_id ON audit_events (actor_id);
CREATE INDEX idx_audit_events_initiator_id ON audit_events (initiator_id);
CREATE INDEX idx_audit_events_repo_id ON audit_events (repo_id);

CREATE TRIGGER audit_events_set_updated_at
BEFORE UPDATE ON audit_events
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

ALTER TABLE deployments ADD COLUMN rollback_of_id UUID REFERENCES deployments (id);

CREATE INDEX idx_deployments_rollback_of_id ON deployments (rollback_of_id);
