-- Time-Travel Debug recording index. Each row is a pointer to a JSON
-- file in the content-addressed store; the file is identified by the
-- sha256 `id`. Filtering happens here in SQL; replay reads the file.
CREATE TABLE IF NOT EXISTS recordings (
  id TEXT PRIMARY KEY,
  ts TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL,
  service_id TEXT,
  route_id TEXT,
  consumer TEXT,
  bytes_request INTEGER NOT NULL,
  bytes_response INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_recordings_ts ON recordings(ts);
CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);
