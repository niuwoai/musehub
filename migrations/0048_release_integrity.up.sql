ALTER TABLE releases
    ADD COLUMN IF NOT EXISTS create_request_sha256 CHAR(64);

CREATE UNIQUE INDEX IF NOT EXISTS uk_releases_repo_tag_name
    ON releases (repo_id, tag_name);
