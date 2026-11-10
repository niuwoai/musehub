ALTER TABLE releases DROP CONSTRAINT IF EXISTS releases_oci_image_ref_format;
ALTER TABLE releases DROP COLUMN IF EXISTS oci_image_ref;
