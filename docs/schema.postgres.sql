-- Lynxx Postgres schema (assembled from migrations/postgres/*.sql)
-- Generated 2026-05-06 from the same files sqlx applies on billing service startup.


-- ============================================================================
-- 0001_initial.sql
-- ============================================================================
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

-- ============================================================================
-- 0002_admin_auth.sql
-- ============================================================================
CREATE TABLE IF NOT EXISTS admin_tokens (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  expires_at TEXT,
  last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);

-- audit_log is append-only with a hash chain — no is_deleted (entries
-- can't be soft-deleted without invalidating downstream chain hashes).
CREATE TABLE IF NOT EXISTS audit_log (
  id BIGSERIAL PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  ts TEXT NOT NULL,
  actor_id TEXT,
  actor_name TEXT,
  actor_role TEXT,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  diff JSONB,
  hash TEXT NOT NULL,
  prev_hash TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts);
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);

-- ============================================================================
-- 0003_tls_columns.sql
-- ============================================================================
-- Per-service TLS configuration. See migrations/sqlite/0006_tls_columns.sql
-- for the rationale; this is the Postgres counterpart.
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_verify BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_client_cert_path TEXT;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_client_key_path TEXT;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_ca_path TEXT;

-- ============================================================================
-- 0004_recordings.sql
-- ============================================================================
-- Time-Travel Debug recording index. See sqlite/0007 for the rationale.
CREATE TABLE IF NOT EXISTS recordings (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  ts TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status SMALLINT NOT NULL,
  duration_ms BIGINT NOT NULL,
  service_id TEXT,
  route_id TEXT,
  consumer TEXT,
  bytes_request BIGINT NOT NULL,
  bytes_response BIGINT NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE INDEX IF NOT EXISTS idx_recordings_ts ON recordings(ts DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);

-- ============================================================================
-- 0008_quota.sql
-- ============================================================================
-- Monthly API call quota tracking.
CREATE TABLE IF NOT EXISTS quota_usage (
  period          TEXT    NOT NULL PRIMARY KEY,  -- 'YYYY-MM'
  calls_total     BIGINT  NOT NULL DEFAULT 0,
  calls_rejected  BIGINT  NOT NULL DEFAULT 0,
  updated_at      TEXT NOT NULL
);

-- ============================================================================
-- 0009_billing.sql
-- ============================================================================
-- Billing schema: customers, API keys, and Stripe linkage.
-- Both tables follow the standard metadata convention: internal id +
-- external urn + created_at + updated_at + is_deleted.

CREATE TABLE IF NOT EXISTS customers (
  id                     TEXT PRIMARY KEY,
  urn                    TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  email                  TEXT NOT NULL UNIQUE,
  password_hash          TEXT NOT NULL,
  name                   TEXT,
  company                TEXT,
  tier                   TEXT NOT NULL DEFAULT 'free',
  stripe_customer_id     TEXT,
  stripe_subscription_id TEXT,
  subscription_status    TEXT NOT NULL DEFAULT 'active',
  is_deleted             BOOLEAN NOT NULL DEFAULT FALSE,
  created_at             TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at             TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);

CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);
CREATE INDEX IF NOT EXISTS idx_customers_urn ON customers(urn);

CREATE TABLE IF NOT EXISTS api_keys (
  id            TEXT PRIMARY KEY,
  urn           TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  customer_id   TEXT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  name          TEXT NOT NULL,
  key_hash      TEXT NOT NULL UNIQUE,
  key_prefix    TEXT NOT NULL,
  status        TEXT NOT NULL DEFAULT 'active',
  expires_at    TEXT,
  is_deleted    BOOLEAN NOT NULL DEFAULT FALSE,
  created_at    TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at    TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  last_used_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_keys_customer ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_urn ON api_keys(urn);
