CREATE TABLE repositories (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    slug TEXT NOT NULL,
    slug_normalized TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    default_branch TEXT NOT NULL DEFAULT 'main',
    storage_key UUID NOT NULL,
    object_format TEXT NOT NULL DEFAULT 'sha1' CHECK (object_format = 'sha1'),
    state TEXT NOT NULL CHECK (state IN ('creating', 'ready', 'failed', 'archived')) DEFAULT 'creating',
    next_issue_number BIGINT NOT NULL DEFAULT 1,
    next_pr_number BIGINT NOT NULL DEFAULT 1,
    row_version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_repositories_org_slug UNIQUE (org_id, slug_normalized),
    CONSTRAINT uk_repositories_storage_key UNIQUE (storage_key)
);

CREATE INDEX idx_repositories_org_id ON repositories (org_id);
CREATE INDEX idx_repositories_state ON repositories (state);

CREATE TRIGGER repositories_set_updated_at
BEFORE UPDATE ON repositories
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE repository_grants (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    repo_id UUID NOT NULL REFERENCES repositories (id),
    principal_id UUID NOT NULL REFERENCES principals (id),
    role TEXT NOT NULL CHECK (role IN ('reader', 'developer', 'maintainer')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_repository_grants_repo_principal UNIQUE (repo_id, principal_id)
);

CREATE INDEX idx_repository_grants_org_id ON repository_grants (org_id);
CREATE INDEX idx_repository_grants_repo_id ON repository_grants (repo_id);
CREATE INDEX idx_repository_grants_principal_id ON repository_grants (principal_id);

CREATE TRIGGER repository_grants_set_updated_at
BEFORE UPDATE ON repository_grants
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
