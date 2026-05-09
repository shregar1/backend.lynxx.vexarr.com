-- Commercial Zero-Trust Extensions

-- 1. Organizations (rename/refine customers)
-- We keep 'customers' for now as it maps to Stripe, but we'll add 'workspaces' 
-- if we want to support multiple users per account properly.
-- For v1 "Majestic Monolith", we'll stick to a 1:1 user:customer model 
-- or extend customers to be Organizations.

-- 2. Device Fingerprints
CREATE TABLE IF NOT EXISTS device_fingerprints (
  id           UUID PRIMARY KEY,
  customer_id  BIGINT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  hardware_id  TEXT NOT NULL, -- The fingerprint
  name         TEXT, -- User-assigned name
  os_info      TEXT,
  is_blocked   BOOLEAN NOT NULL DEFAULT FALSE,
  last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(customer_id, hardware_id)
);

-- 3. Runtime Grants (for session management and revocation)
-- PASETO is stateless, but we store the metadata here to allow instant revocation.
CREATE TABLE IF NOT EXISTS runtime_grants (
  id           UUID PRIMARY KEY,
  customer_id  BIGINT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  device_id    UUID REFERENCES device_fingerprints(id) ON DELETE SET NULL,
  token_id     TEXT NOT NULL UNIQUE, -- JTI or similar unique identifier in the token
  expires_at   TIMESTAMPTZ NOT NULL,
  revoked_at   TIMESTAMPTZ,
  last_heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 4. Commercial Audit Logs
-- Extending the existing audit_log logic but for end-user actions.
CREATE TABLE IF NOT EXISTS user_activity_log (
  id            BIGSERIAL PRIMARY KEY,
  customer_id   BIGINT NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
  actor_id      BIGINT REFERENCES api_keys(id), -- If action was via API key
  action        TEXT NOT NULL, -- e.g. 'premium_feature_executed'
  resource      TEXT,
  ip_address    INET,
  metadata      JSONB,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_device_fingerprints_customer ON device_fingerprints(customer_id);
CREATE INDEX IF NOT EXISTS idx_runtime_grants_customer ON runtime_grants(customer_id);
CREATE INDEX IF NOT EXISTS idx_runtime_grants_token ON runtime_grants(token_id);
CREATE INDEX IF NOT EXISTS idx_user_activity_customer ON user_activity_log(customer_id, created_at DESC);

-- Trigger for device_fingerprints updated_at
CREATE TRIGGER trg_device_fingerprints_bump_updated_at 
BEFORE UPDATE ON device_fingerprints
FOR EACH ROW EXECUTE FUNCTION bump_updated_at();
