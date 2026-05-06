-- Remove redundant TEXT enum columns from billing tables.
-- The FK columns (billing_tier_id, subscription_status_id,
-- subscription_type_id, api_key_status_id) are now the sole
-- source of truth. Business logic JOINs the lookup tables to
-- resolve human-readable codes.

ALTER TABLE customers DROP COLUMN IF EXISTS tier;
ALTER TABLE customers DROP COLUMN IF EXISTS subscription_status;
ALTER TABLE api_keys  DROP COLUMN IF EXISTS status;
