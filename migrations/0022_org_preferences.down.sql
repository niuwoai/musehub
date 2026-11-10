ALTER TABLE organizations DROP CONSTRAINT IF EXISTS chk_org_preferences_object;
ALTER TABLE organizations DROP COLUMN IF EXISTS preferences;
