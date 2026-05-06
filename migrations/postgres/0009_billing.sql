-- Billing schema: customers, API keys, and Stripe linkage.
-- Both tables follow the standard metadata convention: internal id +
-- external urn + created_at + updated_at + is_deleted.
--
-- FK columns (`*_id`) reference lookup tables created in 0001_initial.sql
-- and are NOT NULL so every row resolves cleanly via JOIN.

CREATE TABLE IF NOT EXISTS customers (
  id                     TEXT PRIMARY KEY,
  urn                    TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  email                  TEXT NOT NULL UNIQUE,
  password_hash          TEXT NOT NULL,
  name                   TEXT,
  company                TEXT,
  tier                   TEXT NOT NULL DEFAULT 'free',
  -- 1 = free (the first row in billing_tier_lk).
  billing_tier_id                INTEGER NOT NULL DEFAULT 1 REFERENCES billing_tier_lk(id),
  stripe_customer_id     TEXT,
  stripe_subscription_id TEXT,
  subscription_status    TEXT NOT NULL DEFAULT 'active',
  -- 1 = active in subscription_status_lk.
  subscription_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_status_lk(id),
  -- 1 = free in subscription_type_lk (free-forever; no billing relationship).
  subscription_type_id   INTEGER NOT NULL DEFAULT 1 REFERENCES subscription_type_lk(id),
  is_deleted             BOOLEAN NOT NULL DEFAULT FALSE,
  created_at             TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at             TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);

CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);
CREATE INDEX IF NOT EXISTS idx_customers_urn ON customers(urn);
CREATE INDEX IF NOT EXISTS idx_customers_billing_tier ON customers(billing_tier_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_status ON customers(subscription_status_id);
CREATE INDEX IF NOT EXISTS idx_customers_subscription_type ON customers(subscription_type_id);

CREATE TABLE IF NOT EXISTS api_keys (
  id                TEXT PRIMARY KEY,
  urn               TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  customer_id       TEXT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  name              TEXT NOT NULL,
  key_hash          TEXT NOT NULL UNIQUE,
  key_prefix        TEXT NOT NULL,
  status            TEXT NOT NULL DEFAULT 'active',
  -- 1 = active in api_key_status_lk.
  api_key_status_id INTEGER NOT NULL DEFAULT 1 REFERENCES api_key_status_lk(id),
  expires_at        TEXT,
  is_deleted        BOOLEAN NOT NULL DEFAULT FALSE,
  created_at        TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at        TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  last_used_at      TEXT
);

CREATE INDEX IF NOT EXISTS idx_api_keys_customer ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_urn ON api_keys(urn);
CREATE INDEX IF NOT EXISTS idx_api_keys_status ON api_keys(api_key_status_id);
