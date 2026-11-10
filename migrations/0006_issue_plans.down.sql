DROP TABLE IF EXISTS implementation_plans;
ALTER TABLE issues DROP COLUMN IF EXISTS current_revision_id;
DROP TABLE IF EXISTS issue_revisions;
