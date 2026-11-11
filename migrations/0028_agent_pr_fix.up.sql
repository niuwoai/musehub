-- S3: agent fix rounds anchored to PR head (docs/01-product-requirements.md §2 S3).

ALTER TABLE agent_runs
    ADD COLUMN pr_id UUID REFERENCES pull_requests (id),
    ADD COLUMN head_oid CHAR(40),
    ADD COLUMN fix_round INT NOT NULL DEFAULT 1;

CREATE INDEX idx_agent_runs_pr_id ON agent_runs (repo_id, pr_id, created_at);
