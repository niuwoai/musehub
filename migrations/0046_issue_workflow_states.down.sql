-- Preserve row state instead of silently collapsing newer states during rollback.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM issues WHERE state IN ('in_review', 'resolved')) THEN
        RAISE EXCEPTION 'cannot roll back 0046 while issues use in_review or resolved';
    END IF;
END
$$;

ALTER TABLE issues DROP CONSTRAINT IF EXISTS issues_state_check;
ALTER TABLE issues
    ADD CONSTRAINT issues_state_check
    CHECK (state IN ('open', 'in_progress', 'closed'));
