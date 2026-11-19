ALTER TABLE pipeline_workflow_job_attempts
    ADD COLUMN result JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT ck_pipeline_workflow_job_attempts_result_object
        CHECK (jsonb_typeof(result) = 'object');

ALTER TABLE artifacts
    DROP CONSTRAINT uk_artifacts_run_name,
    ADD COLUMN workflow_attempt_id UUID
        REFERENCES pipeline_workflow_job_attempts (id) ON DELETE CASCADE;

ALTER TABLE artifacts DROP CONSTRAINT artifacts_kind_check;
ALTER TABLE artifacts ADD CONSTRAINT artifacts_kind_check
    CHECK (kind IN ('file', 'junit', 'oci_image'));

CREATE UNIQUE INDEX uk_artifacts_run_name_without_attempt
    ON artifacts (pipeline_run_id, name)
    WHERE workflow_attempt_id IS NULL;
CREATE UNIQUE INDEX uk_artifacts_attempt_name
    ON artifacts (workflow_attempt_id, name)
    WHERE workflow_attempt_id IS NOT NULL;
CREATE INDEX idx_artifacts_workflow_attempt
    ON artifacts (workflow_attempt_id, state, name);
