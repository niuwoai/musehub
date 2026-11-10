-- Persist agent task text for remote workers (API enqueue-only).
ALTER TABLE agent_runs
    ADD COLUMN task_message TEXT NOT NULL DEFAULT '';
