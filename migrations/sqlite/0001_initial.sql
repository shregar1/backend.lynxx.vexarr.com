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
  ('hash',              'Consistent Hash',   'Hash a request attribute (header, IP, etc.) to a target.');

-- ---------------------------------------------------------------------------
-- Business tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS services (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
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
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  service_id INTEGER NOT NULL,
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
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  urn TEXT NOT NULL UNIQUE DEFAULT (
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))),2) || '-' ||
    lower(hex(randomblob(6)))
  ),
  name TEXT NOT NULL,
  service_id INTEGER,
  route_id INTEGER,
  enabled INTEGER NOT NULL DEFAULT 1,
  config TEXT NOT NULL,
  is_deleted INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE,
  FOREIGN KEY (route_id) REFERENCES routes(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS consumers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
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
