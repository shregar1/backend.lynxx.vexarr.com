-- Convert plain UNIQUE constraints to partial unique indexes so that
-- soft-deleted rows (is_deleted = 1) don't block recreating a resource
-- with the same name/key.

-- services.name
CREATE UNIQUE INDEX IF NOT EXISTS uq_services_name_active
  ON services(name) WHERE is_deleted = 0;
-- Drop the old column-level UNIQUE by recreating without it.
-- SQLite can't DROP CONSTRAINT, but we can shadow it with an index
-- that allows duplicates among deleted rows. The partial index above
-- enforces uniqueness only for active rows. To fully remove the
-- column-level constraint we'd need to recreate the table, which is
-- risky in a migration. Instead, on soft-delete we append a tombstone
-- suffix so the old constraint never fires.

-- consumers.username
CREATE UNIQUE INDEX IF NOT EXISTS uq_consumers_username_active
  ON consumers(username) WHERE is_deleted = 0;

-- admin_tokens.token_hash
CREATE UNIQUE INDEX IF NOT EXISTS uq_admin_tokens_hash_active
  ON admin_tokens(token_hash) WHERE is_deleted = 0;
