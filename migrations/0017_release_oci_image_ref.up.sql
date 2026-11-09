-- Bind published releases to deployable OCI image reference (digest-pinned).
ALTER TABLE releases ADD COLUMN oci_image_ref TEXT;

ALTER TABLE releases ADD CONSTRAINT releases_oci_image_ref_format
    CHECK (
        oci_image_ref IS NULL
        OR (
            oci_image_ref ~ '@sha256:[a-f0-9]{64}$'
            AND length(oci_image_ref) <= 512
        )
    );
