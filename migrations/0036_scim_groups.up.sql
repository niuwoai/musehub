CREATE TABLE scim_groups (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations (id),
    external_id TEXT CHECK (external_id IS NULL OR length(external_id) > 0),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT uk_scim_groups_org_id_id UNIQUE (org_id, id),
    CONSTRAINT uk_scim_groups_org_external_id UNIQUE (org_id, external_id)
);

CREATE INDEX idx_scim_groups_org_id ON scim_groups (org_id);
CREATE TRIGGER scim_groups_set_updated_at
BEFORE UPDATE ON scim_groups
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();

CREATE TABLE scim_group_members (
    org_id UUID NOT NULL,
    group_id UUID NOT NULL,
    principal_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (org_id, group_id, principal_id),
    CONSTRAINT fk_scim_group_members_group FOREIGN KEY (org_id, group_id)
        REFERENCES scim_groups (org_id, id) ON DELETE CASCADE,
    CONSTRAINT fk_scim_group_members_identity FOREIGN KEY (org_id, principal_id)
        REFERENCES scim_identities (org_id, principal_id) ON DELETE CASCADE
);

CREATE INDEX idx_scim_group_members_org_principal ON scim_group_members (org_id, principal_id);
CREATE TRIGGER scim_group_members_set_updated_at
BEFORE UPDATE ON scim_group_members
FOR EACH ROW
EXECUTE FUNCTION musehub_set_updated_at();
