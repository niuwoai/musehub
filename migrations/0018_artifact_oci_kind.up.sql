ALTER TABLE artifacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'file';
ALTER TABLE artifacts ADD COLUMN oci_image_ref TEXT;

ALTER TABLE artifacts ADD CONSTRAINT artifacts_kind_check
    CHECK (kind IN ('file', 'oci_image'));

ALTER TABLE artifacts ADD CONSTRAINT artifacts_oci_image_ref_format
    CHECK (
        oci_image_ref IS NULL
        OR (
            oci_image_ref ~ '@sha256:[a-f0-9]{64}$'
            AND length(oci_image_ref) <= 512
        )
    );
