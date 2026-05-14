CREATE TABLE IF NOT EXISTS logs (
    id          UUID PRIMARY KEY,
    urn         TEXT NOT NULL UNIQUE DEFAULT gen_random_uuid()::text,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    level       TEXT NOT NULL DEFAULT 'info',
    message     TEXT NOT NULL DEFAULT '',
    service_id  UUID,
    route_id    UUID,
    project_id  UUID,
    method      TEXT,
    path        TEXT,
    status      INTEGER,
    duration_ms BIGINT,
    client_ip   TEXT,
    metadata    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_logs_ts         ON logs(ts DESC);
CREATE INDEX IF NOT EXISTS idx_logs_service_id ON logs(service_id);
CREATE INDEX IF NOT EXISTS idx_logs_route_id   ON logs(route_id);
CREATE INDEX IF NOT EXISTS idx_logs_project_id ON logs(project_id);
CREATE INDEX IF NOT EXISTS idx_logs_level      ON logs(level);
CREATE INDEX IF NOT EXISTS idx_logs_urn        ON logs(urn);
