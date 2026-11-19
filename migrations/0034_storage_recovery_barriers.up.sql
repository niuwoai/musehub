CREATE TABLE storage_recovery_barriers (
    barrier_id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_storage_recovery_barriers_created_at
    ON storage_recovery_barriers (created_at);

CREATE TRIGGER storage_recovery_barriers_set_updated_at
BEFORE UPDATE ON storage_recovery_barriers
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
