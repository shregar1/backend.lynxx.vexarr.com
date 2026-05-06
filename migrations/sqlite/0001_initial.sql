-- Standard columns on every business table:
--   id          internal primary key (never exposed externally)
--   urn         external identifier (UUID4 string, used by every API)
--   created_at  ISO-8601 timestamp at insert
--   updated_at  ISO-8601 timestamp, refreshed on every UPDATE
--   is_deleted  soft-delete flag; reads filter by `is_deleted = 0`
--
-- The `urn` column has a SQLite expression default that produces a
-- UUID4-formatted string, so existing INSERTs that don't specify urn
-- still get one without code changes.

CREATE TABLE IF NOT EXISTS services (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  name TEXT NOT NULL UNIQUE,
  url TEXT NOT NULL,
  retries INTEGER NOT NULL DEFAULT 3,
  connect_timeout_ms INTEGER NOT NULL DEFAULT 5000,
  enabled INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS routes (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  service_id TEXT NOT NULL,
  paths TEXT NOT NULL,
  methods TEXT NOT NULL,
  hosts TEXT NOT NULL,
  strip_path INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS plugins (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  name TEXT NOT NULL,
  service_id TEXT,
  route_id TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  config TEXT NOT NULL,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE,
  FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS consumers (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  username TEXT NOT NULL UNIQUE,
  custom_id TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_routes_service ON routes(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_service ON plugins(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_route ON plugins(route_id);
