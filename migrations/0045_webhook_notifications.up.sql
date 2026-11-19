ALTER TABLE repositories
    ADD CONSTRAINT uk_repositories_org_id_id UNIQUE (org_id, id);

CREATE TABLE webhook_subscriptions (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    repo_id UUID,
    endpoint_url TEXT NOT NULL,
    encrypted_secret BYTEA NOT NULL,
    event_types TEXT[] NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_by UUID NOT NULL REFERENCES principals (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT fk_webhook_subscriptions_repo_org FOREIGN KEY (org_id, repo_id)
        REFERENCES repositories (org_id, id) ON DELETE CASCADE,
    CONSTRAINT ck_webhook_subscriptions_event_types CHECK (cardinality(event_types) BETWEEN 1 AND 16),
    CONSTRAINT ck_webhook_subscriptions_https CHECK (endpoint_url LIKE 'https://%')
);

CREATE INDEX idx_webhook_subscriptions_org ON webhook_subscriptions (org_id, enabled);
CREATE INDEX idx_webhook_subscriptions_org_repo ON webhook_subscriptions (org_id, repo_id, enabled);
CREATE INDEX idx_webhook_subscriptions_created_by ON webhook_subscriptions (created_by);

CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY,
    subscription_id UUID NOT NULL REFERENCES webhook_subscriptions (id) ON DELETE CASCADE,
    audit_event_id UUID NOT NULL REFERENCES audit_events (id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'delivering', 'delivered', 'failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    locked_until TIMESTAMPTZ,
    response_status INTEGER,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (subscription_id, audit_event_id)
);

CREATE INDEX idx_webhook_deliveries_due ON webhook_deliveries (available_at, created_at)
    WHERE status IN ('pending', 'delivering');
CREATE INDEX idx_webhook_deliveries_subscription ON webhook_deliveries (subscription_id, created_at DESC);
CREATE INDEX idx_webhook_deliveries_audit_event ON webhook_deliveries (audit_event_id);

CREATE TRIGGER webhook_subscriptions_set_updated_at
BEFORE UPDATE ON webhook_subscriptions
FOR EACH ROW EXECUTE FUNCTION musehub_set_updated_at();

CREATE TRIGGER webhook_deliveries_set_updated_at
BEFORE UPDATE ON webhook_deliveries
FOR EACH ROW EXECUTE FUNCTION musehub_set_updated_at();

CREATE FUNCTION musehub_enqueue_webhook_deliveries() RETURNS trigger AS $$
BEGIN
    IF NEW.org_id IS NULL THEN
        RETURN NEW;
    END IF;
    INSERT INTO webhook_deliveries (id, subscription_id, audit_event_id)
    SELECT gen_random_uuid(), s.id, NEW.id
    FROM webhook_subscriptions s
    WHERE s.org_id = NEW.org_id
      AND s.enabled
      AND (s.repo_id IS NULL OR s.repo_id = NEW.repo_id)
      AND NEW.action = ANY(s.event_types)
    ON CONFLICT (subscription_id, audit_event_id) DO NOTHING;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_events_enqueue_webhooks
AFTER INSERT ON audit_events
FOR EACH ROW EXECUTE FUNCTION musehub_enqueue_webhook_deliveries();
