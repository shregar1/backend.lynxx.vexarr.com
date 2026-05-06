-- Per-service TLS configuration. See migrations/sqlite/0006_tls_columns.sql
-- for the rationale; this is the Postgres counterpart.
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_verify BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_client_cert_path TEXT;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_client_key_path TEXT;
ALTER TABLE services ADD COLUMN IF NOT EXISTS tls_ca_path TEXT;
