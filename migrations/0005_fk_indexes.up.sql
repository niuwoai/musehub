-- BOOT-02: foreign keys must have leading-column indexes (check_schema).
CREATE INDEX idx_issues_author_id ON issues (author_id);
CREATE INDEX idx_issues_repo_id ON issues (repo_id);
CREATE INDEX idx_agent_runs_org_id ON agent_runs (org_id);
CREATE INDEX idx_agent_runs_repo_id ON agent_runs (repo_id);
CREATE INDEX idx_agent_runs_requested_by ON agent_runs (requested_by);
