DROP TRIGGER IF EXISTS audit_events_enqueue_webhooks ON audit_events;
DROP FUNCTION IF EXISTS musehub_enqueue_webhook_deliveries();
DROP TABLE IF EXISTS webhook_deliveries;
DROP TABLE IF EXISTS webhook_subscriptions;
ALTER TABLE repositories DROP CONSTRAINT IF EXISTS uk_repositories_org_id_id;
