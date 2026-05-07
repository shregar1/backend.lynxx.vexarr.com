-- Lynxx schema — SQLite DDL (parity with migrations/postgres/001_ddl.sql).
--
-- Used in dev / unit tests / single-node deployments. Type mappings:
--   BIGSERIAL    -> INTEGER PRIMARY KEY AUTOINCREMENT
--   TIMESTAMPTZ  -> TEXT (ISO-8601, set via datetime('now'))
--   BOOLEAN      -> INTEGER (0/1)
--   TEXT[]       -> TEXT (JSON array, e.g. '["read","write"]')
--   JSONB        -> TEXT (JSON)
--   gen_random_uuid() -> randomblob-based v4 UUID expression
--
-- SQLite triggers can't easily replace urn defaults across all backends,
-- so we keep the inline DEFAULT expression here. Application code is
-- the source of truth for urn generation — this just gives sqlite a
-- safe fallback when a row is inserted without one.
--
-- Gateway tables (services, routes, plugins, consumers, targets,
-- admin_tokens, audit_log, recordings) use `id TEXT PRIMARY KEY`
-- holding a UUID4 string — the gateway's repository layer mints a
-- Uuid in Rust and binds it as the row's external id. This is the
-- pre-consolidation behaviour; the cross-cutting `urn` convention
-- only applies to lookup (`*_lk`) and billing (customers, api_keys)
-- tables. See `src/db/sqlite.rs` for the binding code.

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
  monthly_call_limit INTEGER,
  is_deleted         INTEGER NOT NULL DEFAULT 0,
  created_at         TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

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

-- ---------------------------------------------------------------------------
-- Business tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS services (
  id                   TEXT PRIMARY KEY,
  name                 TEXT NOT NULL UNIQUE,
  url                  TEXT NOT NULL,
  retries              INTEGER NOT NULL DEFAULT 3,
  connect_timeout_ms   INTEGER NOT NULL DEFAULT 5000,
  enabled              INTEGER NOT NULL DEFAULT 1,
  lb_strategy          TEXT NOT NULL DEFAULT 'round-robin',
  -- 1 = round-robin (the first row seeded into lb_strategy_lk).
  lb_strategy_id       INTEGER NOT NULL DEFAULT 1 REFERENCES lb_strategy_lk(id),
  tls_verify           INTEGER NOT NULL DEFAULT 1,
  tls_client_cert_path TEXT,
  tls_client_key_path  TEXT,
  tls_ca_path          TEXT,
  is_deleted           INTEGER NOT NULL DEFAULT 0,
  created_at           TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS routes (
  id         TEXT PRIMARY KEY,
  service_id TEXT NOT NULL,
  paths      TEXT NOT NULL,
  methods    TEXT NOT NULL,
  hosts      TEXT NOT NULL DEFAULT '[]',
  strip_path INTEGER NOT NULL DEFAULT 1,
  enabled    INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS plugins (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  service_id TEXT,
  route_id   TEXT,
  enabled    INTEGER NOT NULL DEFAULT 1,
  config     TEXT NOT NULL,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE,
  FOREIGN KEY (route_id)   REFERENCES routes(id)   ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS consumers (
  id         TEXT PRIMARY KEY,
  username   TEXT NOT NULL UNIQUE,
  custom_id  TEXT,
  api_key    TEXT,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS targets (
  id         TEXT PRIMARY KEY,
  service_id TEXT NOT NULL,
  url        TEXT NOT NULL,
  weight     INTEGER NOT NULL DEFAULT 1,
  enabled    INTEGER NOT NULL DEFAULT 1,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS admin_tokens (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  role         TEXT NOT NULL,
  -- 3 = viewer (read-only); the safest default. Code that creates
  -- elevated tokens must set user_type_id explicitly.
  user_type_id INTEGER NOT NULL DEFAULT 3 REFERENCES user_type_lk(id),
  is_deleted   INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
  expires_at   TEXT,
  last_used_at TEXT
);

CREATE TABLE IF NOT EXISTS audit_log (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  ts            TEXT NOT NULL,
  -- Holds the actor's admin_tokens.id (UUID string), not a numeric id.
  actor_id      TEXT,
  actor_name    TEXT,
  actor_role    TEXT,
  action        TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id   TEXT,
  diff          TEXT,
  hash          TEXT NOT NULL,
  prev_hash     TEXT,
  created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS recordings (
  id             TEXT PRIMARY KEY,
  ts             TEXT NOT NULL,
  method         TEXT NOT NULL,
  path           TEXT NOT NULL,
  status         INTEGER NOT NULL,
  duration_ms    INTEGER NOT NULL,
  service_id     TEXT,
  route_id       TEXT,
  consumer       TEXT,
  bytes_request  INTEGER NOT NULL,
  bytes_response INTEGER NOT NULL,
  is_deleted     INTEGER NOT NULL DEFAULT 0,
  created_at     TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS quota_usage (
  period         TEXT    NOT NULL PRIMARY KEY,
  calls_total    INTEGER NOT NULL DEFAULT 0,
  calls_rejected INTEGER NOT NULL DEFAULT 0,
  updated_at     TEXT    NOT NULL
);

-- _sync_log is a sqlite-only fallback for cluster sync (postgres uses
-- LISTEN/NOTIFY). Each row is a serialized ChangeEvent.
CREATE TABLE IF NOT EXISTS _sync_log (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  payload    TEXT    NOT NULL,
  created_at TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS customers (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  urn                    TEXT NOT NULL UNIQUE DEFAULT (
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
  -- 1 = free in billing_tier_lk.
  billing_tier_id        INTEGER NOT NULL DEFAULT 1 REFERENCES billing_tier_lk(id),
  stripe_customer_id     TEXT,
  stripe_subscription_id TEXT,
  -- 1 = active in subscription_status_lk.
  subscription_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_status_lk(id),
  -- 1 = free in subscription_type_lk.
  subscription_type_id   INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_type_lk(id),
  is_deleted             INTEGER NOT NULL DEFAULT 0,
  created_at             TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS api_keys (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  urn               TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  customer_id       INTEGER NOT NULL,
  name              TEXT NOT NULL,
  key_hash          TEXT NOT NULL UNIQUE,
  key_prefix        TEXT NOT NULL,
  -- 1 = active in api_key_status_lk.
  api_key_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES api_key_status_lk(id),
  -- JSON-encoded array of scopes (sqlite has no TEXT[]).
  scopes            TEXT NOT NULL DEFAULT '[]',
  expires_at        TEXT,
  revoked_at        TEXT,
  is_deleted        INTEGER NOT NULL DEFAULT 0,
  created_at        TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
  last_used_at      TEXT,
  FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE
);

-- ---------------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------------

CREATE INDEX        IF NOT EXISTS idx_routes_service                ON routes(service_id);
CREATE INDEX        IF NOT EXISTS idx_plugins_service               ON plugins(service_id);
CREATE INDEX        IF NOT EXISTS idx_plugins_route                 ON plugins(route_id);
CREATE INDEX        IF NOT EXISTS idx_targets_service               ON targets(service_id);
CREATE INDEX        IF NOT EXISTS idx_services_lb_strategy          ON services(lb_strategy_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_consumers_api_key             ON consumers(api_key) WHERE api_key IS NOT NULL;
CREATE INDEX        IF NOT EXISTS idx_admin_tokens_hash             ON admin_tokens(token_hash);
CREATE INDEX        IF NOT EXISTS idx_admin_tokens_user_type        ON admin_tokens(user_type_id);
CREATE INDEX        IF NOT EXISTS idx_audit_ts                      ON audit_log(ts);
CREATE INDEX        IF NOT EXISTS idx_audit_resource                ON audit_log(resource_type, resource_id);
CREATE INDEX        IF NOT EXISTS idx_recordings_ts                 ON recordings(ts);
CREATE INDEX        IF NOT EXISTS idx_recordings_path               ON recordings(path);
CREATE INDEX        IF NOT EXISTS idx_recordings_status             ON recordings(status);
CREATE INDEX        IF NOT EXISTS idx_sync_log_seq                  ON _sync_log(seq);
CREATE INDEX        IF NOT EXISTS idx_customers_stripe_customer     ON customers(stripe_customer_id);
CREATE INDEX        IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);
CREATE INDEX        IF NOT EXISTS idx_customers_urn                 ON customers(urn);
CREATE INDEX        IF NOT EXISTS idx_customers_billing_tier        ON customers(billing_tier_id);
CREATE INDEX        IF NOT EXISTS idx_customers_subscription_status ON customers(subscription_status_id);
CREATE INDEX        IF NOT EXISTS idx_customers_subscription_type   ON customers(subscription_type_id);
CREATE INDEX        IF NOT EXISTS idx_api_keys_customer             ON api_keys(customer_id);
CREATE INDEX        IF NOT EXISTS idx_api_keys_hash                 ON api_keys(key_hash);
CREATE INDEX        IF NOT EXISTS idx_api_keys_urn                  ON api_keys(urn);
CREATE INDEX        IF NOT EXISTS idx_api_keys_status               ON api_keys(api_key_status_id);

-- ---------------------------------------------------------------------------
-- Triggers — bump updated_at on UPDATE for every standard table.
-- For the gateway business tables, `id` is a TEXT UUID minted in Rust
-- (no urn column). For lookup + billing tables, urn is set inline via
-- column DEFAULT. audit_log + quota_usage + _sync_log are exempt
-- (no updated_at).
-- ---------------------------------------------------------------------------

CREATE TRIGGER IF NOT EXISTS trg_user_type_lk_bump_updated_at
  AFTER UPDATE ON user_type_lk FOR EACH ROW
  BEGIN UPDATE user_type_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_billing_tier_lk_bump_updated_at
  AFTER UPDATE ON billing_tier_lk FOR EACH ROW
  BEGIN UPDATE billing_tier_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_subscription_status_lk_bump_updated_at
  AFTER UPDATE ON subscription_status_lk FOR EACH ROW
  BEGIN UPDATE subscription_status_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_subscription_type_lk_bump_updated_at
  AFTER UPDATE ON subscription_type_lk FOR EACH ROW
  BEGIN UPDATE subscription_type_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_api_key_status_lk_bump_updated_at
  AFTER UPDATE ON api_key_status_lk FOR EACH ROW
  BEGIN UPDATE api_key_status_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_lb_strategy_lk_bump_updated_at
  AFTER UPDATE ON lb_strategy_lk FOR EACH ROW
  BEGIN UPDATE lb_strategy_lk SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_services_bump_updated_at
  AFTER UPDATE ON services FOR EACH ROW
  BEGIN UPDATE services SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_routes_bump_updated_at
  AFTER UPDATE ON routes FOR EACH ROW
  BEGIN UPDATE routes SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_plugins_bump_updated_at
  AFTER UPDATE ON plugins FOR EACH ROW
  BEGIN UPDATE plugins SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_consumers_bump_updated_at
  AFTER UPDATE ON consumers FOR EACH ROW
  BEGIN UPDATE consumers SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_targets_bump_updated_at
  AFTER UPDATE ON targets FOR EACH ROW
  BEGIN UPDATE targets SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_admin_tokens_bump_updated_at
  AFTER UPDATE ON admin_tokens FOR EACH ROW
  BEGIN UPDATE admin_tokens SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_recordings_bump_updated_at
  AFTER UPDATE ON recordings FOR EACH ROW
  BEGIN UPDATE recordings SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_customers_bump_updated_at
  AFTER UPDATE ON customers FOR EACH ROW
  BEGIN UPDATE customers SET updated_at = datetime('now') WHERE id = NEW.id; END;
CREATE TRIGGER IF NOT EXISTS trg_api_keys_bump_updated_at
  AFTER UPDATE ON api_keys FOR EACH ROW
  BEGIN UPDATE api_keys SET updated_at = datetime('now') WHERE id = NEW.id; END;
