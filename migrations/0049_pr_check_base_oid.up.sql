ALTER TABLE pipeline_runs ADD COLUMN pr_base_oid CHAR(40);
ALTER TABLE pr_check_runs ADD COLUMN base_oid CHAR(40);

UPDATE pr_check_runs checks
SET base_oid = prs.base_oid
FROM pull_requests prs
WHERE prs.id = checks.pr_id;
