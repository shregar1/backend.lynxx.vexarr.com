CREATE TABLE IF NOT EXISTS admin_tokens (
  id BIGSERIAL PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL,
  -- 3 = viewer (read-only); the safest default. Code that creates
  -- elevated tokens must set user_type_id explicitly.
  user_type_id INTEGER NOT NULL DEFAULT 3 REFERENCES user_type_lk(id),
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ,
  last_used_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_admin_tokens_user_type ON admin_tokens(user_type_id);

-- audit_log is append-only with a hash chain — no is_deleted (entries
-- can't be soft-deleted without invalidating downstream chain hashes).
CREATE TABLE IF NOT EXISTS audit_log (
  id BIGSERIAL PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  ts TIMESTAMPTZ NOT NULL,
  actor_id BIGINT,
  actor_name TEXT,
  actor_role TEXT,
  action TEXT NOT NULL,
  resource_type TEXT NOT NULL,
  resource_id TEXT,
  diff JSONB,
  hash TEXT NOT NULL,
  prev_hash TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts);
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);
