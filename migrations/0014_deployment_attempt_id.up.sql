-- Idempotent deployment starts: same repo + attempt_id replays existing row.
ALTER TABLE deployments ADD COLUMN attempt_id UUID;

CREATE UNIQUE INDEX uk_deployments_repo_attempt
    ON deployments (repo_id, attempt_id)
    WHERE attempt_id IS NOT NULL;
