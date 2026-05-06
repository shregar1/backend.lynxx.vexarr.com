-- Standard columns on every business table:
--   id          internal primary key (never exposed externally)
--   urn         external identifier (UUID4, used by every API)
--   created_at  ISO-8601 string at insert
--   updated_at  ISO-8601 string refreshed on every UPDATE
--   is_deleted  soft-delete flag; reads filter by `is_deleted = false`
--
-- Lookup tables (`*_lk`-style closed sets) live at the top of this file
-- so business tables below can reference them in their original CREATE
-- with NOT NULL FKs. Each lookup row has:
--   id          SERIAL — referenced as the FK on business tables
--   urn         standard external identifier
--   code        machine-readable string the application reads/writes
--   label       human-readable display name
--   description short prose explaining what the row means
-- Seeded inserts assign deterministic ids (1, 2, …) — the FK defaults
-- below rely on those ids.

-- ---------------------------------------------------------------------------
-- Lookup tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS user_type_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO user_type_lk (code, label, description) VALUES
  ('admin',    'Admin',    'Full control over the gateway, including token management.'),
  ('operator', 'Operator', 'Day-to-day operations: services, routes, plugins, recordings.'),
  ('viewer',   'Viewer',   'Read-only access for dashboards and audits.'),
  ('internal', 'Internal', 'Internal company developer; full scope of every subscription tier.')
ON CONFLICT (code) DO NOTHING;

CREATE TABLE IF NOT EXISTS billing_tier_lk (
  id                 SERIAL PRIMARY KEY,
  urn                TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code               TEXT NOT NULL UNIQUE,
  label              TEXT NOT NULL,
  description        TEXT NOT NULL,
  monthly_call_limit BIGINT,                       -- NULL = unlimited
  is_deleted         BOOLEAN NOT NULL DEFAULT FALSE,
  created_at         TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at         TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO billing_tier_lk (code, label, description, monthly_call_limit) VALUES
  ('free',       'Free',       '1M calls/month · self-host · API key included.',                  1000000),
  ('pro',        'Pro',        '10M calls/month · clustering · 99.9% SLA.',                       10000000),
  ('business',   'Business',   'Unlimited calls · SSO · audit log retention · 99.99% SLA.',       NULL),
  ('enterprise', 'Enterprise', 'Custom contract, dedicated support, on-prem options.',            NULL)
ON CONFLICT (code) DO NOTHING;

CREATE TABLE IF NOT EXISTS subscription_status_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO subscription_status_lk (code, label, description) VALUES
  ('active',   'Active',   'Subscription is in good standing.'),
  ('past_due', 'Past Due', 'Most recent invoice failed; in grace period.'),
  ('canceled', 'Canceled', 'Subscription terminated; API keys revoked.')
ON CONFLICT (code) DO NOTHING;

-- subscription_type_lk is orthogonal to subscription_status_lk: status
-- tracks the lifecycle (active/past_due/canceled), type tracks how the
-- customer obtained the subscription (paid via Stripe, trial, comp,
-- internal-company-use, free-forever).
CREATE TABLE IF NOT EXISTS subscription_type_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO subscription_type_lk (code, label, description) VALUES
  ('free',     'Free',     'Free-forever tier; no billing relationship.'),
  ('trial',    'Trial',    '14-day Pro trial; converts to paid on first card charge.'),
  ('paid',     'Paid',     'Stripe-billed subscription with active payment method.'),
  ('comp',     'Comp',     'Complimentary subscription granted by sales / support.'),
  ('internal', 'Internal', 'Issued to internal company developers with unlimited scope.')
ON CONFLICT (code) DO NOTHING;

CREATE TABLE IF NOT EXISTS api_key_status_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO api_key_status_lk (code, label, description) VALUES
  ('active',  'Active',  'Key is live and verifies successfully.'),
  ('revoked', 'Revoked', 'Key was deactivated by the customer or by Stripe webhook.'),
  ('expired', 'Expired', 'Key passed its expires_at timestamp.')
ON CONFLICT (code) DO NOTHING;

CREATE TABLE IF NOT EXISTS lb_strategy_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at  TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
INSERT INTO lb_strategy_lk (code, label, description) VALUES
  ('round-robin',       'Round Robin',       'Cycle through targets in order.'),
  ('weighted',          'Weighted',          'Distribute proportionally to per-target weights.'),
  ('least-connections', 'Least Connections', 'Pick the target with the fewest in-flight requests.'),
  ('hash',              'Consistent Hash',   'Hash a request attribute (header, IP, etc.) to a target.')
ON CONFLICT (code) DO NOTHING;

-- ---------------------------------------------------------------------------
-- Business tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS services (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  name TEXT NOT NULL UNIQUE,
  url TEXT NOT NULL,
  retries BIGINT NOT NULL DEFAULT 3,
  connect_timeout_ms BIGINT NOT NULL DEFAULT 5000,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  lb_strategy TEXT NOT NULL DEFAULT 'round-robin',
  -- 1 = round-robin (the first row inserted into lb_strategy_lk above).
  lb_strategy_id INTEGER NOT NULL DEFAULT 1 REFERENCES lb_strategy_lk(id),
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
CREATE INDEX IF NOT EXISTS idx_services_lb_strategy ON services(lb_strategy_id);
