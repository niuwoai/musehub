DROP INDEX IF EXISTS uk_releases_repo_tag_name;

ALTER TABLE releases
    DROP COLUMN IF EXISTS create_request_sha256;
