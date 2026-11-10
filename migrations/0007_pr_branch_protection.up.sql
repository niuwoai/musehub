-- Default branch protection metadata (enforced also by git pre-receive hook).
ALTER TABLE repositories
    ADD COLUMN deny_direct_default_push BOOL NOT NULL DEFAULT true;

CREATE OR REPLACE FUNCTION musehub_plan_accepted_immutable()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.state = 'accepted' THEN
        IF NEW.body IS DISTINCT FROM OLD.body
            OR NEW.issue_revision_id IS DISTINCT FROM OLD.issue_revision_id
            OR NEW.version IS DISTINCT FROM OLD.version
        THEN
            RAISE EXCEPTION 'accepted implementation plan is immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER implementation_plans_accepted_immutable
BEFORE UPDATE ON implementation_plans
FOR EACH ROW
EXECUTE FUNCTION musehub_plan_accepted_immutable();

CREATE TABLE pull_requests (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    number BIGINT NOT NULL,
    author_id UUID NOT NULL REFERENCES principals (id),
    title TEXT NOT NULL,
    source_branch TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'merged', 'closed')) DEFAULT 'open',
    issue_id UUID REFERENCES issues (id),
    agent_run_id UUID REFERENCES agent_runs (id),
    head_oid CHAR(40),
    merged_oid CHAR(40),
    merged_by UUID REFERENCES principals (id),
    merged_at TIMESTAMPTZ,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_pull_requests_repo_number UNIQUE (repo_id, number)
);

CREATE INDEX idx_pull_requests_org_id ON pull_requests (org_id);
CREATE INDEX idx_pull_requests_repo_id ON pull_requests (repo_id);
CREATE INDEX idx_pull_requests_author_id ON pull_requests (author_id);
CREATE INDEX idx_pull_requests_issue_id ON pull_requests (issue_id);

CREATE TRIGGER pull_requests_set_updated_at
BEFORE UPDATE ON pull_requests
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE pr_reviews (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    pr_id UUID NOT NULL REFERENCES pull_requests (id),
    reviewer_id UUID NOT NULL REFERENCES principals (id),
    decision TEXT NOT NULL CHECK (decision IN ('approve', 'request_changes', 'comment')),
    body TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_pr_reviews_org_id ON pr_reviews (org_id);
CREATE INDEX idx_pr_reviews_repo_id ON pr_reviews (repo_id);
CREATE INDEX idx_pr_reviews_pr_id ON pr_reviews (pr_id);
CREATE INDEX idx_pr_reviews_reviewer_id ON pr_reviews (reviewer_id);

CREATE TRIGGER pr_reviews_set_updated_at
BEFORE UPDATE ON pr_reviews
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
