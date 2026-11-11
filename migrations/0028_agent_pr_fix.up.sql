-- S3: agent fix rounds anchored to PR head (docs/01-product-requirements.md §2 S3).

ALTER TABLE agent_runs
    ADD COLUMN pr_id UUID REFERENCES pull_requests (id),
    ADD COLUMN head_oid CHAR(40),
    ADD COLUMN fix_round INT NOT NULL DEFAULT 1;

-- BOOT-02: FK agent_runs(pr_id) needs an index whose leading column is pr_id.
CREATE INDEX idx_agent_runs_pr_id_leading ON agent_runs (pr_id);
CREATE INDEX idx_agent_runs_pr_id ON agent_runs (repo_id, pr_id, created_at);

CREATE UNIQUE INDEX uk_agent_runs_pr_fix_round
    ON agent_runs (pr_id, fix_round) WHERE pr_id IS NOT NULL;

CREATE UNIQUE INDEX uk_agent_runs_pr_inflight
    ON agent_runs (pr_id)
    WHERE pr_id IS NOT NULL
      AND state IN ('queued', 'running', 'cancel_requested', 'waiting_input');
