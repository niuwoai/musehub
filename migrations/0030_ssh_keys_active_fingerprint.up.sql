-- Active SSH key fingerprints stay globally unique; revoked keys may be re-added.
ALTER TABLE ssh_keys DROP CONSTRAINT IF EXISTS uk_ssh_keys_fingerprint;
DROP INDEX IF EXISTS uk_ssh_keys_fingerprint_active;
CREATE UNIQUE INDEX uk_ssh_keys_fingerprint_active
    ON ssh_keys (fingerprint) WHERE revoked_at IS NULL;
