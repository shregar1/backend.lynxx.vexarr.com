-- Projects table
CREATE TABLE IF NOT EXISTS projects (
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);

CREATE OR REPLACE FUNCTION trg_projects_bump_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at := NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_projects_bump_updated_at ON projects;
CREATE TRIGGER trg_projects_bump_updated_at
BEFORE UPDATE ON projects
FOR EACH ROW
EXECUTE FUNCTION trg_projects_bump_updated_at();

-- Add project_id, domain, subdomain to services
ALTER TABLE services ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE services ADD COLUMN IF NOT EXISTS domain     TEXT NOT NULL DEFAULT '';
ALTER TABLE services ADD COLUMN IF NOT EXISTS subdomain  TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_services_project_id ON services(project_id);
