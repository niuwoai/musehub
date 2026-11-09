DROP INDEX IF EXISTS uk_deployments_repo_attempt;
ALTER TABLE deployments DROP COLUMN IF EXISTS attempt_id;
