-- Time-Travel Debug recording index. See sqlite/0007 for the rationale.
CREATE TABLE IF NOT EXISTS recordings (
  id TEXT PRIMARY KEY,
  urn TEXT NOT NULL UNIQUE DEFAULT (gen_random_uuid()::text),
  ts TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status SMALLINT NOT NULL,
  duration_ms BIGINT NOT NULL,
  service_id TEXT,
  route_id TEXT,
  consumer TEXT,
  bytes_request BIGINT NOT NULL,
  bytes_response BIGINT NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"')),
  updated_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"+00:00"'))
);
CREATE INDEX IF NOT EXISTS idx_recordings_ts ON recordings(ts DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);
