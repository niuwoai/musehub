DROP INDEX IF EXISTS uk_ssh_keys_fingerprint_active;
DELETE FROM ssh_keys s1
USING ssh_keys s2
WHERE s1.fingerprint = s2.fingerprint
  AND s1.id > s2.id;
ALTER TABLE ssh_keys ADD CONSTRAINT uk_ssh_keys_fingerprint UNIQUE (fingerprint);
