-- Projects table
CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    is_deleted  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);

CREATE TRIGGER IF NOT EXISTS trg_projects_bump_updated_at
AFTER UPDATE ON projects
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE projects SET updated_at = datetime('now') WHERE id = NEW.id;
END;

-- Add project_id, domain, subdomain to services
ALTER TABLE services ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE services ADD COLUMN domain     TEXT NOT NULL DEFAULT '';
ALTER TABLE services ADD COLUMN subdomain  TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_services_project_id ON services(project_id);
