DROP TABLE release_artifacts;

ALTER TABLE releases
    DROP COLUMN release_notes;

ALTER TABLE agent_runs
    DROP COLUMN recent_progress;
