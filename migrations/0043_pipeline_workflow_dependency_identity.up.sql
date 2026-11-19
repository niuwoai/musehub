ALTER TABLE pipeline_workflow_job_dependencies
    ADD COLUMN id UUID;

UPDATE pipeline_workflow_job_dependencies
SET id = gen_random_uuid()
WHERE id IS NULL;

ALTER TABLE pipeline_workflow_job_dependencies
    ALTER COLUMN id SET NOT NULL,
    ALTER COLUMN id SET DEFAULT gen_random_uuid(),
    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    DROP CONSTRAINT pk_pipeline_workflow_job_dependencies,
    ADD CONSTRAINT pk_pipeline_workflow_job_dependencies PRIMARY KEY (id),
    ADD CONSTRAINT uk_pipeline_workflow_job_dependencies_job_needs
        UNIQUE (job_id, needs_job_id);

CREATE TRIGGER pipeline_workflow_job_dependencies_set_updated_at
BEFORE UPDATE ON pipeline_workflow_job_dependencies
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
