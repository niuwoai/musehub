CREATE TABLE issue_revisions (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    issue_id UUID NOT NULL REFERENCES issues (id),
    revision INT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    author_id UUID NOT NULL REFERENCES principals (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_issue_revisions_issue_revision UNIQUE (issue_id, revision)
);

CREATE INDEX idx_issue_revisions_org_id ON issue_revisions (org_id);
CREATE INDEX idx_issue_revisions_repo_id ON issue_revisions (repo_id);
CREATE INDEX idx_issue_revisions_issue_id ON issue_revisions (issue_id);
CREATE INDEX idx_issue_revisions_author_id ON issue_revisions (author_id);

CREATE TRIGGER issue_revisions_set_updated_at
BEFORE UPDATE ON issue_revisions
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

ALTER TABLE issues ADD COLUMN current_revision_id UUID REFERENCES issue_revisions (id);

CREATE INDEX idx_issues_current_revision_id ON issues (current_revision_id);

CREATE TABLE implementation_plans (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    issue_revision_id UUID NOT NULL REFERENCES issue_revisions (id),
    version INT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL CHECK (state IN ('draft', 'accepted', 'superseded')) DEFAULT 'draft',
    accepted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_implementation_plans_revision_version UNIQUE (issue_revision_id, version)
);

CREATE INDEX idx_implementation_plans_org_id ON implementation_plans (org_id);
CREATE INDEX idx_implementation_plans_repo_id ON implementation_plans (repo_id);
CREATE INDEX idx_implementation_plans_revision_id ON implementation_plans (issue_revision_id);

CREATE TRIGGER implementation_plans_set_updated_at
BEFORE UPDATE ON implementation_plans
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
