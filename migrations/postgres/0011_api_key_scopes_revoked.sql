-- Add scopes and revoked_at to api_keys per the API Key Management spec.
-- scopes: TEXT[] of permitted operations (e.g. read, write, admin).
-- revoked_at: timestamp of revocation event (NULL while active).

ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS scopes TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS revoked_at TIMESTAMPTZ;
