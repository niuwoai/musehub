ALTER TABLE organizations
    ADD COLUMN preferences JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE organizations
    ADD CONSTRAINT chk_org_preferences_object
    CHECK (jsonb_typeof(preferences) = 'object');
