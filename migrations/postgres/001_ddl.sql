-- Lynxx schema — PostgreSQL DDL.
--
-- Lookup tables (`*_lk`) and billing tables (customers, api_keys) follow
-- the standard column convention:
--   id          BIGSERIAL primary key (internal, never exposed externally)
--   urn         external identifier (UUID4 string, used by every API)
--   created_at  TIMESTAMPTZ at insert
--   updated_at  TIMESTAMPTZ refreshed on every UPDATE (via trigger)
--   is_deleted  BOOLEAN soft-delete flag; reads filter by `is_deleted = false`
--
-- Gateway business tables (services, routes, plugins, consumers, targets,
-- admin_tokens, audit_log, recordings) use `id UUID PRIMARY KEY` minted
-- in Rust as the external id — the gateway repository binds Uuid
-- directly. The cross-cutting urn convention does not apply here.
--
-- Triggers (defined at the bottom):
--   set_urn          fills NEW.urn with a UUID4 if NULL/empty on INSERT
--                    (lookup + billing tables only)
--   bump_updated_at  refreshes NEW.updated_at to now() on UPDATE
-- audit_log (append-only, no updated_at) and quota_usage (single-row PK)
-- are exempt.

-- ---------------------------------------------------------------------------
-- Trigger functions (idempotent — safe to re-run during recovery)
-- ---------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION set_urn() RETURNS trigger AS $$
BEGIN
  IF NEW.urn IS NULL OR NEW.urn = '' THEN
    NEW.urn := gen_random_uuid()::text;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION bump_updated_at() RETURNS trigger AS $$
BEGIN
  NEW.updated_at := now();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ---------------------------------------------------------------------------
-- Lookup tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS user_type_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE,
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS billing_tier_lk (
  id                 SERIAL PRIMARY KEY,
  urn                TEXT NOT NULL UNIQUE,
  code               TEXT NOT NULL UNIQUE,
  label              TEXT NOT NULL,
  description        TEXT NOT NULL,
  monthly_call_limit BIGINT,                       -- NULL = unlimited
  is_deleted         BOOLEAN NOT NULL DEFAULT FALSE,
  created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS subscription_status_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE,
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- subscription_type_lk is orthogonal to subscription_status_lk: status
-- tracks the lifecycle (active/past_due/canceled), type tracks how the
-- customer obtained the subscription (paid via Stripe, trial, comp,
-- internal-company-use, free-forever).
CREATE TABLE IF NOT EXISTS subscription_type_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE,
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_key_status_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE,
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS lb_strategy_lk (
  id          SERIAL PRIMARY KEY,
  urn         TEXT NOT NULL UNIQUE,
  code        TEXT NOT NULL UNIQUE,
  label       TEXT NOT NULL,
  description TEXT NOT NULL,
  is_deleted  BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Business tables
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS services (
  id                   UUID PRIMARY KEY,
  name                 TEXT NOT NULL UNIQUE,
  url                  TEXT NOT NULL,
  retries              BIGINT NOT NULL DEFAULT 3,
  connect_timeout_ms   BIGINT NOT NULL DEFAULT 5000,
  enabled              BOOLEAN NOT NULL DEFAULT TRUE,
  -- Legacy free-form code, kept in sync with lb_strategy_id during code
  -- migration. New code joins lb_strategy_lk via lb_strategy_id.
  lb_strategy          TEXT NOT NULL DEFAULT 'round-robin',
  -- 1 = round-robin (the first row seeded into lb_strategy_lk).
  lb_strategy_id       INTEGER NOT NULL DEFAULT 1 REFERENCES lb_strategy_lk(id),
  -- Per-service mTLS toggles for outbound (gateway → upstream) calls.
  tls_verify           BOOLEAN NOT NULL DEFAULT TRUE,
  tls_client_cert_path TEXT,
  tls_client_key_path  TEXT,
  tls_ca_path          TEXT,
  is_deleted           BOOLEAN NOT NULL DEFAULT FALSE,
  created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS routes (
  id         UUID PRIMARY KEY,
  service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
  paths      JSONB NOT NULL,
  methods    JSONB NOT NULL,
  hosts      JSONB NOT NULL DEFAULT '[]'::jsonb,
  strip_path BOOLEAN NOT NULL DEFAULT TRUE,
  enabled    BOOLEAN NOT NULL DEFAULT TRUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS plugins (
  id         UUID PRIMARY KEY,
  name       TEXT NOT NULL,
  service_id UUID REFERENCES services(id) ON DELETE CASCADE,
  route_id   UUID REFERENCES routes(id) ON DELETE CASCADE,
  enabled    BOOLEAN NOT NULL DEFAULT TRUE,
  config     JSONB NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS consumers (
  id         UUID PRIMARY KEY,
  username   TEXT NOT NULL UNIQUE,
  custom_id  TEXT,
  api_key    TEXT UNIQUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS targets (
  id         UUID PRIMARY KEY,
  service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
  url        TEXT NOT NULL,
  weight     BIGINT NOT NULL DEFAULT 1,
  enabled    BOOLEAN NOT NULL DEFAULT TRUE,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS admin_tokens (
  id           UUID PRIMARY KEY,
  name         TEXT NOT NULL,
  token_hash   TEXT NOT NULL UNIQUE,
  role         TEXT NOT NULL,
  -- 3 = viewer (read-only); the safest default. Code that creates
  -- elevated tokens must set user_type_id explicitly.
  user_type_id INTEGER NOT NULL DEFAULT 3 REFERENCES user_type_lk(id),
  is_deleted   BOOLEAN NOT NULL DEFAULT FALSE,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at   TIMESTAMPTZ,
  last_used_at TIMESTAMPTZ
);

-- audit_log is append-only with a hash chain — no is_deleted, no
-- updated_at (entries can't be modified without invalidating the chain).
-- `id` is a monotonic BIGSERIAL; external consumers walk the chain by
-- timestamp / hash rather than by exposed row id.
CREATE TABLE IF NOT EXISTS audit_log (
  id            BIGSERIAL PRIMARY KEY,
  ts            TIMESTAMPTZ NOT NULL,
  -- UUID of the actor's admin_tokens.id (the gateway uses UUIDs here too).
  actor_id      UUID,
  actor_name    TEXT,
  actor_role    TEXT,
  action        TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id   TEXT,
  diff          JSONB,
  hash          TEXT NOT NULL,
  prev_hash     TEXT,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS recordings (
  id             TEXT PRIMARY KEY,
  ts             TIMESTAMPTZ NOT NULL,
  method         TEXT NOT NULL,
  path           TEXT NOT NULL,
  status         SMALLINT NOT NULL,
  duration_ms    BIGINT NOT NULL,
  service_id     UUID,
  route_id       UUID,
  consumer       TEXT,
  bytes_request  BIGINT NOT NULL,
  bytes_response BIGINT NOT NULL,
  is_deleted     BOOLEAN NOT NULL DEFAULT FALSE,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- quota_usage is non-standard: one row per calendar month, no urn or
-- soft-delete (counters are reset by inserting next month's row).
CREATE TABLE IF NOT EXISTS quota_usage (
  period         TEXT    NOT NULL PRIMARY KEY,  -- 'YYYY-MM'
  calls_total    BIGINT  NOT NULL DEFAULT 0,
  calls_rejected BIGINT  NOT NULL DEFAULT 0,
  updated_at     TIMESTAMPTZ NOT NULL
);

-- Billing schema — customers + api_keys.
-- All FK *_id columns reference *_lk lookup tables and are NOT NULL so
-- every row resolves cleanly via JOIN. The legacy TEXT enum columns
-- (tier, subscription_status, status) were dropped in favour of the FKs.
CREATE TABLE IF NOT EXISTS customers (
  id                     BIGSERIAL PRIMARY KEY,
  urn                    TEXT NOT NULL UNIQUE,
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
  is_deleted             BOOLEAN NOT NULL DEFAULT FALSE,
  created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS api_keys (
  id                BIGSERIAL PRIMARY KEY,
  urn               TEXT NOT NULL UNIQUE,
  customer_id       BIGINT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  name              TEXT NOT NULL,
  key_hash          TEXT NOT NULL UNIQUE,
  key_prefix        TEXT NOT NULL,
  -- 1 = active in api_key_status_lk.
  api_key_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES api_key_status_lk(id),
  -- TEXT[] of permitted operations (e.g. read, write, admin).
  scopes            TEXT[] NOT NULL DEFAULT '{}',
  expires_at        TIMESTAMPTZ,
  revoked_at        TIMESTAMPTZ,
  is_deleted        BOOLEAN NOT NULL DEFAULT FALSE,
  created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at      TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- Indexes
-- ---------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS idx_routes_service                    ON routes(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_service                   ON plugins(service_id);
CREATE INDEX IF NOT EXISTS idx_plugins_route                     ON plugins(route_id);
CREATE INDEX IF NOT EXISTS idx_targets_service                   ON targets(service_id);
CREATE INDEX IF NOT EXISTS idx_services_lb_strategy              ON services(lb_strategy_id);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash                 ON admin_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_user_type            ON admin_tokens(user_type_id);
CREATE INDEX IF NOT EXISTS idx_audit_ts                          ON audit_log(ts);
CREATE INDEX IF NOT EXISTS idx_audit_resource                    ON audit_log(resource_type, resource_id);
CREATE INDEX IF NOT EXISTS idx_recordings_ts                     ON recordings(ts DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_path                   ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status                 ON recordings(status);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer         ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription     ON customers(stripe_subscription_id);
CREATE INDEX IF NOT EXISTS idx_customers_urn                     ON customers(urn);
CREATE INDEX IF NOT EXISTS idx_customers_billing_tier            ON customers(billing_tier_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_status     ON customers(subscription_status_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_type       ON customers(subscription_type_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_customer                 ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash                     ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_urn                      ON api_keys(urn);
CREATE INDEX IF NOT EXISTS idx_api_keys_status                   ON api_keys(api_key_status_id);

-- ---------------------------------------------------------------------------
-- Triggers
--
-- set_urn applies to lookup + billing tables only (the gateway business
-- tables key on UUID id, no separate urn column). bump_updated_at applies
-- to every table with an updated_at column. audit_log + quota_usage are
-- opted out.
-- ---------------------------------------------------------------------------

DO $$
DECLARE
  t TEXT;
  -- Tables that carry both urn + updated_at columns.
  urn_tables TEXT[] := ARRAY[
    'user_type_lk', 'billing_tier_lk', 'subscription_status_lk',
    'subscription_type_lk', 'api_key_status_lk', 'lb_strategy_lk',
    'customers', 'api_keys'
  ];
  -- Gateway business tables: bump_updated_at only.
  gateway_tables TEXT[] := ARRAY[
    'services', 'routes', 'plugins', 'consumers', 'targets',
    'admin_tokens', 'recordings'
  ];
BEGIN
  FOREACH t IN ARRAY urn_tables LOOP
    EXECUTE format('DROP TRIGGER IF EXISTS trg_%I_set_urn ON %I', t, t);
    EXECUTE format('CREATE TRIGGER trg_%I_set_urn BEFORE INSERT ON %I
                    FOR EACH ROW EXECUTE FUNCTION set_urn()', t, t);
    EXECUTE format('DROP TRIGGER IF EXISTS trg_%I_bump_updated_at ON %I', t, t);
    EXECUTE format('CREATE TRIGGER trg_%I_bump_updated_at BEFORE UPDATE ON %I
                    FOR EACH ROW EXECUTE FUNCTION bump_updated_at()', t, t);
  END LOOP;

  FOREACH t IN ARRAY gateway_tables LOOP
    EXECUTE format('DROP TRIGGER IF EXISTS trg_%I_bump_updated_at ON %I', t, t);
    EXECUTE format('CREATE TRIGGER trg_%I_bump_updated_at BEFORE UPDATE ON %I
                    FOR EACH ROW EXECUTE FUNCTION bump_updated_at()', t, t);
  END LOOP;
END $$;
