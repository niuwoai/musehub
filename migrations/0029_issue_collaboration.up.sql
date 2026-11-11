-- ISS-01: assignee, labels, close reason on issues.

ALTER TABLE issues
    ADD COLUMN IF NOT EXISTS assignee_id UUID REFERENCES principals (id),
    ADD COLUMN IF NOT EXISTS close_reason TEXT,
    ADD COLUMN IF NOT EXISTS labels TEXT[] NOT NULL DEFAULT '{}';

CREATE INDEX IF NOT EXISTS idx_issues_repo_number_desc ON issues (repo_id, number DESC);
CREATE INDEX IF NOT EXISTS idx_issues_labels ON issues USING GIN (labels);
CREATE INDEX IF NOT EXISTS idx_issues_assignee ON issues (repo_id, assignee_id) WHERE assignee_id IS NOT NULL;
