DROP TABLE IF EXISTS deployment_approvals;
DROP TABLE IF EXISTS pipeline_job_logs;
DROP TABLE IF EXISTS agent_run_messages;
DROP TABLE IF EXISTS discussion_comments;
DROP TABLE IF EXISTS issue_acceptance_criteria;
ALTER TABLE implementation_plans DROP COLUMN IF EXISTS steps;
ALTER TABLE agent_runs DROP CONSTRAINT IF EXISTS agent_runs_state_check;
ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_state_check CHECK (state IN (
        'queued',
        'running',
        'succeeded',
        'failed',
        'cancel_requested',
        'canceled'
    ));
