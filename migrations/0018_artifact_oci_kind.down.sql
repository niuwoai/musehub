ALTER TABLE artifacts DROP CONSTRAINT IF EXISTS artifacts_oci_image_ref_format;
ALTER TABLE artifacts DROP CONSTRAINT IF EXISTS artifacts_kind_check;
ALTER TABLE artifacts DROP COLUMN IF EXISTS oci_image_ref;
ALTER TABLE artifacts DROP COLUMN IF EXISTS kind;
