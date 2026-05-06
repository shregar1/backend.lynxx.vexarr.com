-- Add scopes and revoked_at to api_keys per the API Key Management spec.
-- SQLite lacks TEXT[]; store scopes as JSON text (e.g. '["read","write"]').
-- revoked_at: ISO-8601 timestamp of revocation event (NULL while active).

ALTER TABLE api_keys ADD COLUMN scopes TEXT NOT NULL DEFAULT '[]';
ALTER TABLE api_keys ADD COLUMN revoked_at TEXT;
