DROP TRIGGER pipeline_workflow_job_dependencies_set_updated_at
    ON pipeline_workflow_job_dependencies;

ALTER TABLE pipeline_workflow_job_dependencies
    DROP CONSTRAINT uk_pipeline_workflow_job_dependencies_job_needs,
    DROP CONSTRAINT pk_pipeline_workflow_job_dependencies,
    DROP COLUMN updated_at,
    DROP COLUMN id,
    ADD CONSTRAINT pk_pipeline_workflow_job_dependencies
        PRIMARY KEY (job_id, needs_job_id);
