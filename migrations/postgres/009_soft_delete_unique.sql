-- Convert plain UNIQUE constraints to partial unique indexes so that
-- soft-deleted rows don't block recreating a resource with the same name/key.

-- services.name: drop column-level unique, add partial
ALTER TABLE services DROP CONSTRAINT IF EXISTS services_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_services_name_active
  ON services(name) WHERE is_deleted = FALSE;

-- consumers.username
ALTER TABLE consumers DROP CONSTRAINT IF EXISTS consumers_username_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_consumers_username_active
  ON consumers(username) WHERE is_deleted = FALSE;

-- admin_tokens.token_hash
ALTER TABLE admin_tokens DROP CONSTRAINT IF EXISTS admin_tokens_token_hash_key;
CREATE UNIQUE INDEX IF NOT EXISTS uq_admin_tokens_hash_active
  ON admin_tokens(token_hash) WHERE is_deleted = FALSE;
