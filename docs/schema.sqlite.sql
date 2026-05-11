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
-- Lookup tables sit at the top so business CREATE TABLE statements
-- below can reference them with NOT NULL FK columns. Each lookup row
-- has id (INTEGER, used as FK), urn, code, label, description, plus
-- the standard metadata columns. Seeded inserts give deterministic
-- ids so the FK defaults can hardcode them.

-- ---------------------------------------------------------------------------
-- Lookup tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS user_type_lk (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  urn         TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO user_type_lk (code, label, description) VALUES
  ('admin',    'Admin',    'Full control over the gateway, including token management.'),
  ('operator', 'Operator', 'Day-to-day operations: services, routes, plugins, recordings.'),
  ('viewer',   'Viewer',   'Read-only access for dashboards and audits.'),
  ('internal', 'Internal', 'Internal company developer; full scope of every subscription tier.');

CREATE TABLE IF NOT EXISTS billing_tier_lk (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  urn                TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code               TEXT NOT NULL UNIQUE,
  label              TEXT NOT NULL,
  description        TEXT NOT NULL,
  monthly_call_limit INTEGER,                     -- NULL = unlimited
  is_deleted         INTEGER NOT NULL DEFAULT 0,
  created_at         TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO billing_tier_lk (code, label, description, monthly_call_limit) VALUES
  ('free',       'Free',       '1M calls/month · self-host · API key included.',                  1000000),
  ('pro',        'Pro',        '10M calls/month · clustering · 99.9% SLA.',                       10000000),
  ('business',   'Business',   'Unlimited calls · SSO · audit log retention · 99.99% SLA.',       NULL),
  ('enterprise', 'Enterprise', 'Custom contract, dedicated support, on-prem options.',            NULL);

CREATE TABLE IF NOT EXISTS subscription_status_lk (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  urn         TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO subscription_status_lk (code, label, description) VALUES
  ('active',   'Active',   'Subscription is in good standing.'),
  ('past_due', 'Past Due', 'Most recent invoice failed; in grace period.'),
  ('canceled', 'Canceled', 'Subscription terminated; API keys revoked.');

CREATE TABLE IF NOT EXISTS subscription_type_lk (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  urn         TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO subscription_type_lk (code, label, description) VALUES
  ('free',     'Free',     'Free-forever tier; no billing relationship.'),
  ('trial',    'Trial',    '14-day Pro trial; converts to paid on first card charge.'),
  ('paid',     'Paid',     'Stripe-billed subscription with active payment method.'),
  ('comp',     'Comp',     'Complimentary subscription granted by sales / support.'),
  ('internal', 'Internal', 'Issued to internal company developers with unlimited scope.');

CREATE TABLE IF NOT EXISTS api_key_status_lk (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  urn         TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO api_key_status_lk (code, label, description) VALUES
  ('active',  'Active',  'Key is live and verifies successfully.'),
  ('revoked', 'Revoked', 'Key was deactivated by the customer or by Stripe webhook.'),
  ('expired', 'Expired', 'Key passed its expires_at timestamp.');

CREATE TABLE IF NOT EXISTS lb_strategy_lk (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  urn         TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT OR IGNORE INTO lb_strategy_lk (code, label, description) VALUES
  ('round-robin',       'Round Robin',       'Cycle through targets in order.'),
  ('weighted',          'Weighted',          'Distribute proportionally to per-target weights.'),
  ('least-connections', 'Least Connections', 'Pick the target with the fewest in-flight requests.'),
  ('ip-hash',           'IP Hash',           'Rendezvous-hash client IP for consistent target affinity.'),
  ('cookie',            'Cookie Sticky',     'Sticky sessions via _lynxx_upstream cookie; falls back to round-robin.'),
  ('consistent-hash',   'Consistent Hash',   'Rendezvous-hash the request URI for minimal remapping on pool changes.');

-- ---------------------------------------------------------------------------
-- Business tables
-- ---------------------------------------------------------------------------

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
  lb_strategy TEXT NOT NULL DEFAULT 'round-robin',
  -- 1 = round-robin (the first row inserted into lb_strategy_lk above).
  lb_strategy_id INTEGER NOT NULL DEFAULT 1 REFERENCES lb_strategy_lk(id),
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
CREATE INDEX IF NOT EXISTS idx_services_lb_strategy ON services(lb_strategy_id);

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
-- services.lb_strategy + lb_strategy_id are now declared inline in
-- 0001_initial.sql alongside the lb_strategy_lk lookup table.

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
  -- 3 = viewer (read-only); the safest default. Code that creates
  -- elevated tokens must set user_type_id explicitly.
  user_type_id INTEGER NOT NULL DEFAULT 3 REFERENCES user_type_lk(id),
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  expires_at TEXT,
  last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_user_type ON admin_tokens(user_type_id);

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
  email                  TEXT NOT NULL UNIQUE,
  password_hash          TEXT NOT NULL,
  name                   TEXT,
  company                TEXT,
  tier                   TEXT NOT NULL DEFAULT 'free',
  -- 1 = free in billing_tier_lk.
  billing_tier_id        INTEGER NOT NULL DEFAULT 1 REFERENCES billing_tier_lk(id),
  stripe_customer_id     TEXT,
  stripe_subscription_id TEXT,
  subscription_status    TEXT NOT NULL DEFAULT 'active',
  -- 1 = active in subscription_status_lk.
  subscription_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_status_lk(id),
  -- 1 = free in subscription_type_lk (free-forever; no billing relationship).
  subscription_type_id   INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_type_lk(id),
  is_deleted             INTEGER NOT NULL DEFAULT 0,
  created_at             TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);
CREATE INDEX IF NOT EXISTS idx_customers_urn ON customers(urn);
CREATE INDEX IF NOT EXISTS idx_customers_billing_tier ON customers(billing_tier_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_status ON customers(subscription_status_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_type ON customers(subscription_type_id);

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
  customer_id       TEXT NOT NULL,
  name              TEXT NOT NULL,
  key_hash          TEXT NOT NULL UNIQUE,
  key_prefix        TEXT NOT NULL,
  status            TEXT NOT NULL DEFAULT 'active',
  -- 1 = active in api_key_status_lk.
  api_key_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES api_key_status_lk(id),
  expires_at        TEXT,
  is_deleted        INTEGER NOT NULL DEFAULT 0,
  created_at        TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
  last_used_at      TEXT,
  FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_api_keys_customer ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_urn ON api_keys(urn);
CREATE INDEX IF NOT EXISTS idx_api_keys_status ON api_keys(api_key_status_id);
