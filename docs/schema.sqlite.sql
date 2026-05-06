-- Lynxx SQLite schema (assembled from migrations/sqlite/*.sql)
-- Generated 2026-05-06 — same files sqlx applies for local dev.


-- ============================================================================
-- 0001_initial.sql
-- ============================================================================
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

-- ============================================================================
-- 0002_targets_and_lb.sql
-- ============================================================================
CREATE TABLE IF NOT EXISTS targets (
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
  url TEXT NOT NULL,
  weight INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_targets_service ON targets(service_id);
ALTER TABLE services ADD COLUMN lb_strategy TEXT NOT NULL DEFAULT 'round-robin';

-- ============================================================================
-- 0003_consumer_api_key.sql
-- ============================================================================
ALTER TABLE consumers ADD COLUMN api_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_consumers_api_key ON consumers(api_key) WHERE api_key IS NOT NULL;

-- ============================================================================
-- 0004_sync_log.sql
-- ============================================================================
-- Cluster sync log used by the SqliteBus polling fallback. Each row
-- represents a ChangeEvent that nodes consume in monotonically increasing
-- `seq` order. Single-node / dev only — Postgres deployments use LISTEN/NOTIFY
-- instead.
CREATE TABLE IF NOT EXISTS _sync_log (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    payload    TEXT    NOT NULL,
    created_at TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sync_log_seq ON _sync_log(seq);

-- ============================================================================
-- 0005_admin_auth.sql
-- ============================================================================
CREATE TABLE IF NOT EXISTS admin_tokens (
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
  token_hash TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  expires_at TEXT,
  last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);

-- audit_log is append-only with a hash chain — no is_deleted (entries
-- can't be soft-deleted without invalidating downstream chain hashes).
-- urn lets external consumers reference a row without exposing the
-- monotonically-increasing internal id.
CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  ts TEXT NOT NULL,
  actor_id TEXT,
  actor_name TEXT,
  actor_role TEXT,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  diff TEXT,
  hash TEXT NOT NULL,
  prev_hash TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts);
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);

-- ============================================================================
-- 0006_tls_columns.sql
-- ============================================================================
-- Per-service TLS configuration for outbound (gateway -> upstream) mTLS.
--
-- `tls_verify` defaults to 1 (verify) so existing https upstreams keep their
-- current behaviour. The three path columns are NULL by default; when set,
-- the proxy http_client builds a per-service hyper-util client wired with
-- the corresponding cert/key/CA. See `proxy::http_client::ProxyClientPool`.
ALTER TABLE services ADD COLUMN tls_verify INTEGER NOT NULL DEFAULT 1;
ALTER TABLE services ADD COLUMN tls_client_cert_path TEXT;
ALTER TABLE services ADD COLUMN tls_client_key_path TEXT;
ALTER TABLE services ADD COLUMN tls_ca_path TEXT;

-- ============================================================================
-- 0007_recordings.sql
-- ============================================================================
-- Time-Travel Debug recording index. Each row is a pointer to a JSON
-- file in the content-addressed store; the file is identified by the
-- sha256 `id`. Filtering happens here in SQL; replay reads the file.
CREATE TABLE IF NOT EXISTS recordings (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  ts TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  service_id TEXT,
  route_id TEXT,
  consumer TEXT,
  bytes_request INTEGER NOT NULL,
  bytes_response INTEGER NOT NULL,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_recordings_ts ON recordings(ts);
CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);

-- ============================================================================
-- 0008_quota.sql
-- ============================================================================
-- Monthly API call quota tracking.
-- One row per calendar month (YYYY-MM). The gateway reads the current period's
-- row on startup and flushes in-memory counters back every 30 s.
CREATE TABLE IF NOT EXISTS quota_usage (
  period          TEXT    NOT NULL PRIMARY KEY,  -- 'YYYY-MM'
  calls_total     INTEGER NOT NULL DEFAULT 0,
  calls_rejected  INTEGER NOT NULL DEFAULT 0,
  updated_at      TEXT    NOT NULL
);

-- ============================================================================
-- 0009_billing.sql
-- ============================================================================
-- Billing schema: customers, API keys, and Stripe linkage.
-- Used by the lynxx-billing service. Both tables follow the standard
-- metadata convention: internal id + external urn + created_at + updated_at + is_deleted.

CREATE TABLE IF NOT EXISTS customers (
  id                   TEXT PRIMARY KEY,
  urn                  TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  email                TEXT NOT NULL UNIQUE,
  password_hash        TEXT NOT NULL,
  name                 TEXT,
  company              TEXT,
  tier                 TEXT NOT NULL DEFAULT 'free',
  stripe_customer_id   TEXT,
  stripe_subscription_id TEXT,
  subscription_status  TEXT NOT NULL DEFAULT 'active',
  is_deleted           INTEGER NOT NULL DEFAULT 0,
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);
CREATE INDEX IF NOT EXISTS idx_customers_urn ON customers(urn);

CREATE TABLE IF NOT EXISTS api_keys (
  id           TEXT PRIMARY KEY,
  urn          TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  customer_id  TEXT NOT NULL,
  name         TEXT NOT NULL,
  key_hash     TEXT NOT NULL UNIQUE,
  key_prefix   TEXT NOT NULL,
  status       TEXT NOT NULL DEFAULT 'active',
  expires_at   TEXT,
  is_deleted   INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
  last_used_at TEXT,
  FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_api_keys_customer ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_urn ON api_keys(urn);
