-- Billing schema: customers, API keys, and Stripe linkage.
-- Used by the lynxx-billing service (separate binary). The gateway only
-- reads the api_keys table via the GET /v1/license/verify endpoint.

CREATE TABLE IF NOT EXISTS customers (
  id                   TEXT PRIMARY KEY,         -- ULID/UUID string
  email                TEXT NOT NULL UNIQUE,
  password_hash        TEXT NOT NULL,            -- argon2id
  name                 TEXT,
  company              TEXT,
  tier                 TEXT NOT NULL DEFAULT 'free',
  stripe_customer_id   TEXT,
  stripe_subscription_id TEXT,
  subscription_status  TEXT NOT NULL DEFAULT 'active', -- active | past_due | canceled
  created_at           TEXT NOT NULL,
  updated_at           TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_customers_stripe_customer ON customers(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_customers_stripe_subscription ON customers(stripe_subscription_id);

CREATE TABLE IF NOT EXISTS api_keys (
  id           TEXT PRIMARY KEY,
  customer_id  TEXT NOT NULL,
  name         TEXT NOT NULL,
  key_hash     TEXT NOT NULL UNIQUE,             -- sha256 hex of the raw key
  key_prefix   TEXT NOT NULL,                    -- first 16 chars for display
  status       TEXT NOT NULL DEFAULT 'active',   -- active | revoked
  expires_at   TEXT,                             -- NULL = no expiry
  created_at   TEXT NOT NULL,
  last_used_at TEXT,
  FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_api_keys_customer ON api_keys(customer_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
