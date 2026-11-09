ALTER TABLE pr_check_runs
    ADD COLUMN reported_by UUID REFERENCES principals (id),
    ADD COLUMN attempt_id UUID,
    ADD COLUMN expires_at TIMESTAMPTZ;

CREATE INDEX idx_pr_check_runs_reported_by ON pr_check_runs (reported_by);
CREATE INDEX idx_pr_check_runs_attempt_id ON pr_check_runs (attempt_id);
