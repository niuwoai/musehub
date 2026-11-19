DROP INDEX idx_artifacts_workflow_attempt;
DROP INDEX uk_artifacts_attempt_name;
DROP INDEX uk_artifacts_run_name_without_attempt;

ALTER TABLE artifacts
    ADD CONSTRAINT uk_artifacts_run_name UNIQUE (pipeline_run_id, name),
    DROP COLUMN workflow_attempt_id;

ALTER TABLE artifacts DROP CONSTRAINT artifacts_kind_check;
ALTER TABLE artifacts ADD CONSTRAINT artifacts_kind_check
    CHECK (kind IN ('file', 'oci_image'));

ALTER TABLE pipeline_workflow_job_attempts
    DROP CONSTRAINT ck_pipeline_workflow_job_attempts_result_object,
    DROP COLUMN result;
