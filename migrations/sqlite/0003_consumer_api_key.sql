ALTER TABLE consumers ADD COLUMN api_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_consumers_api_key ON consumers(api_key) WHERE api_key IS NOT NULL;
