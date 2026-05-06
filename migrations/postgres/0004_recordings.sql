-- Time-Travel Debug recording index. See sqlite/0007 for the rationale.
CREATE TABLE IF NOT EXISTS recordings (
  id TEXT PRIMARY KEY,
  urn UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
  ts TIMESTAMPTZ NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status SMALLINT NOT NULL,
  duration_ms BIGINT NOT NULL,
  service_id UUID,
  route_id UUID,
  consumer TEXT,
  bytes_request BIGINT NOT NULL,
  bytes_response BIGINT NOT NULL,
  is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_recordings_ts ON recordings(ts DESC);
CREATE INDEX IF NOT EXISTS idx_recordings_path ON recordings(path);
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status);
