-- Standard columns on every business table:
--   id          internal primary key (never exposed externally)
--   urn         external identifier (UUID4, used by every API)
--   created_at  TIMESTAMPTZ at insert
--   updated_at  TIMESTAMPTZ refreshed on every UPDATE
--   is_deleted  soft-delete flag; reads filter by `is_deleted = false`

CREATE TABLE IF NOT EXISTS services (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  name TEXT NOT NULL UNIQUE,
  url TEXT NOT NULL,
  retries BIGINT NOT NULL DEFAULT 3,
  connect_timeout_ms BIGINT NOT NULL DEFAULT 5000,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  lb_strategy TEXT NOT NULL DEFAULT 'round-robin',
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE TABLE IF NOT EXISTS routes (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
  paths JSONB NOT NULL,
  methods JSONB NOT NULL,
  hosts JSONB NOT NULL DEFAULT '[]'::jsonb,
  strip_path BOOLEAN NOT NULL DEFAULT TRUE,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE TABLE IF NOT EXISTS plugins (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  name TEXT NOT NULL,
  service_id TEXT REFERENCES services(id) ON DELETE CASCADE,
  route_id TEXT REFERENCES routes(id) ON DELETE CASCADE,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  config JSONB NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE TABLE IF NOT EXISTS consumers (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  username TEXT NOT NULL UNIQUE,
  custom_id TEXT,
  api_key TEXT UNIQUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE TABLE IF NOT EXISTS targets (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  weight BIGINT NOT NULL DEFAULT 1,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE INDEX IF NOT EXISTS idx_routes_service ON routes(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_service ON plugins(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_route ON plugins(route_id);
CREATE INDEX IF NOT EXISTS idx_targets_service ON targets(service_id);
