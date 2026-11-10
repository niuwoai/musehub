DROP TABLE IF EXISTS pr_check_runs;
DROP TABLE IF EXISTS repo_required_checks;
ALTER TABLE pr_reviews DROP COLUMN IF EXISTS approved_head_oid;
ALTER TABLE pull_requests DROP COLUMN IF EXISTS base_oid;
