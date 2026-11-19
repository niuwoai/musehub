ALTER TABLE agent_runs
    ADD COLUMN recent_progress TEXT;

ALTER TABLE releases
    ADD COLUMN release_notes TEXT NOT NULL DEFAULT '';

CREATE TABLE release_artifacts (
    release_id UUID NOT NULL REFERENCES releases (id) ON DELETE CASCADE,
    artifact_id UUID NOT NULL REFERENCES artifacts (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (release_id, artifact_id)
);

CREATE INDEX idx_release_artifacts_artifact_id ON release_artifacts (artifact_id);
