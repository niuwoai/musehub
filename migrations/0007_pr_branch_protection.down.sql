DROP TABLE IF EXISTS pr_reviews;
DROP TABLE IF EXISTS pull_requests;
DROP TRIGGER IF EXISTS implementation_plans_accepted_immutable ON implementation_plans;
DROP FUNCTION IF EXISTS musehub_plan_accepted_immutable();
ALTER TABLE repositories DROP COLUMN IF EXISTS deny_direct_default_push;
