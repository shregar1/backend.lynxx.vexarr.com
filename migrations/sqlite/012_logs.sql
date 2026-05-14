CREATE TABLE IF NOT EXISTS logs (
    id          TEXT PRIMARY KEY,
    urn         TEXT NOT NULL UNIQUE DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    level       TEXT NOT NULL DEFAULT 'info',
    message     TEXT NOT NULL DEFAULT '',
    service_id  TEXT,
    route_id    TEXT,
    project_id  TEXT,
    method      TEXT,
    path        TEXT,
    status      INTEGER,
    duration_ms INTEGER,
    client_ip   TEXT,
    metadata    TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_logs_ts         ON logs(ts DESC);
CREATE INDEX IF NOT EXISTS idx_logs_service_id ON logs(service_id);
CREATE INDEX IF NOT EXISTS idx_logs_route_id   ON logs(route_id);
CREATE INDEX IF NOT EXISTS idx_logs_project_id ON logs(project_id);
CREATE INDEX IF NOT EXISTS idx_logs_level      ON logs(level);
CREATE INDEX IF NOT EXISTS idx_logs_urn        ON logs(urn);
