DROP INDEX IF EXISTS idx_issues_assignee;
DROP INDEX IF EXISTS idx_issues_labels;
DROP INDEX IF EXISTS idx_issues_repo_number_desc;

ALTER TABLE issues
    DROP COLUMN IF EXISTS labels,
    DROP COLUMN IF EXISTS close_reason,
    DROP COLUMN IF EXISTS assignee_id;
