-- ISS-01: support the documented review and resolution states.
ALTER TABLE issues DROP CONSTRAINT IF EXISTS issues_state_check;
ALTER TABLE issues
    ADD CONSTRAINT issues_state_check
    CHECK (state IN ('open', 'in_progress', 'in_review', 'resolved', 'closed'));
