-- 013: Analytics, Probes, Snapshots, Alerts, IP Rules, Error Pages, Consumer Groups

-- ── Analytics: time-bucketed request metrics ──

CREATE TABLE IF NOT EXISTS metric_buckets (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id  TEXT NOT NULL,
    route_id    TEXT,
    bucket_ts   TEXT NOT NULL,
    granularity TEXT NOT NULL DEFAULT 'minute',
    total_requests  INTEGER NOT NULL DEFAULT 0,
    success_count   INTEGER NOT NULL DEFAULT 0,
    error_4xx       INTEGER NOT NULL DEFAULT 0,
    error_5xx       INTEGER NOT NULL DEFAULT 0,
    total_latency_ms INTEGER NOT NULL DEFAULT 0,
    min_latency_ms   INTEGER,
    max_latency_ms   INTEGER,
    bytes_in    INTEGER NOT NULL DEFAULT 0,
    bytes_out   INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_metric_buckets_service_ts ON metric_buckets(service_id, bucket_ts DESC);
CREATE INDEX IF NOT EXISTS idx_metric_buckets_granularity ON metric_buckets(granularity, bucket_ts);

-- Per-country request aggregates for GeoIP analytics
CREATE TABLE IF NOT EXISTS geo_buckets (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id   TEXT NOT NULL,
    bucket_ts    TEXT NOT NULL,
    country_code TEXT NOT NULL DEFAULT '',
    city         TEXT NOT NULL DEFAULT '',
    request_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_geo_buckets_service_ts ON geo_buckets(service_id, bucket_ts DESC);

-- Rate-limit event aggregates
CREATE TABLE IF NOT EXISTS rate_limit_buckets (
    id           TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id   TEXT NOT NULL,
    consumer_id  TEXT,
    bucket_ts    TEXT NOT NULL,
    allowed      INTEGER NOT NULL DEFAULT 0,
    throttled    INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_buckets_service_ts ON rate_limit_buckets(service_id, bucket_ts DESC);

-- ── Uptime Probes ──

CREATE TABLE IF NOT EXISTS probes (
    id              TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id       TEXT NOT NULL,
    route_id         TEXT,
    name             TEXT NOT NULL,
    method           TEXT NOT NULL DEFAULT 'GET',
    path             TEXT NOT NULL DEFAULT '/',
    interval_seconds INTEGER NOT NULL DEFAULT 60,
    timeout_ms       INTEGER NOT NULL DEFAULT 5000,
    expected_status  INTEGER NOT NULL DEFAULT 200,
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_probes_service ON probes(service_id);

CREATE TABLE IF NOT EXISTS probe_results (
    id         TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    probe_id   TEXT NOT NULL,
    ts         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    status     INTEGER,
    latency_ms INTEGER,
    healthy    INTEGER NOT NULL DEFAULT 1,
    error      TEXT,
    FOREIGN KEY (probe_id) REFERENCES probes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_probe_results_probe_ts ON probe_results(probe_id, ts DESC);

-- ── Snapshots ──

CREATE TABLE IF NOT EXISTS snapshots (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id  TEXT NOT NULL,
    name        TEXT NOT NULL,
    data        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_snapshots_service ON snapshots(service_id, created_at DESC);

-- ── Alert Rules & Events ──

CREATE TABLE IF NOT EXISTS alert_rules (
    id               TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id       TEXT NOT NULL,
    name             TEXT NOT NULL,
    metric           TEXT NOT NULL,
    operator         TEXT NOT NULL DEFAULT 'gt',
    threshold        REAL NOT NULL,
    window_seconds   INTEGER NOT NULL DEFAULT 300,
    cooldown_seconds INTEGER NOT NULL DEFAULT 600,
    channels         TEXT NOT NULL DEFAULT '[]',
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_alert_rules_service ON alert_rules(service_id);

CREATE TABLE IF NOT EXISTS alert_events (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    rule_id     TEXT NOT NULL,
    service_id  TEXT NOT NULL,
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    metric      TEXT NOT NULL,
    value       REAL NOT NULL,
    threshold   REAL NOT NULL,
    message     TEXT NOT NULL DEFAULT '',
    resolved    INTEGER NOT NULL DEFAULT 0,
    resolved_at TEXT,
    FOREIGN KEY (rule_id) REFERENCES alert_rules(id) ON DELETE CASCADE,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_alert_events_service ON alert_events(service_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_alert_events_rule ON alert_events(rule_id, ts DESC);

-- ── IP Rules ──

CREATE TABLE IF NOT EXISTS ip_rules (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id  TEXT NOT NULL,
    route_id    TEXT,
    cidr        TEXT NOT NULL,
    action      TEXT NOT NULL DEFAULT 'deny',
    description TEXT NOT NULL DEFAULT '',
    priority    INTEGER NOT NULL DEFAULT 0,
    enabled     INTEGER NOT NULL DEFAULT 1,
    hits        INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_ip_rules_service ON ip_rules(service_id);

-- ── Error Pages ──

CREATE TABLE IF NOT EXISTS error_pages (
    id          TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id   TEXT NOT NULL,
    status_code  INTEGER NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'text/html',
    body         TEXT NOT NULL DEFAULT '',
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_error_pages_service ON error_pages(service_id);

-- ── Consumer Groups ──

CREATE TABLE IF NOT EXISTS consumer_groups (
    id                  TEXT PRIMARY KEY DEFAULT (
        lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        substr('89ab', abs(random()) % 4 + 1, 1) ||
        substr(lower(hex(randomblob(2))),2) || '-' ||
        lower(hex(randomblob(6)))
    ),
    service_id          TEXT NOT NULL,
    name                TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    rate_limit          INTEGER NOT NULL DEFAULT 100,
    rate_window_seconds INTEGER NOT NULL DEFAULT 60,
    burst_limit         INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_consumer_groups_service ON consumer_groups(service_id);

CREATE TABLE IF NOT EXISTS consumer_group_members (
    group_id    TEXT NOT NULL,
    consumer_id TEXT NOT NULL,
    joined_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (group_id, consumer_id),
    FOREIGN KEY (group_id)    REFERENCES consumer_groups(id) ON DELETE CASCADE,
    FOREIGN KEY (consumer_id) REFERENCES consumers(id) ON DELETE CASCADE
);

-- ── Triggers: auto-update updated_at ──

CREATE TRIGGER IF NOT EXISTS trg_probes_updated
AFTER UPDATE ON probes
BEGIN UPDATE probes SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_alert_rules_updated
AFTER UPDATE ON alert_rules
BEGIN UPDATE alert_rules SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_ip_rules_updated
AFTER UPDATE ON ip_rules
BEGIN UPDATE ip_rules SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_error_pages_updated
AFTER UPDATE ON error_pages
BEGIN UPDATE error_pages SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;

CREATE TRIGGER IF NOT EXISTS trg_consumer_groups_updated
AFTER UPDATE ON consumer_groups
BEGIN UPDATE consumer_groups SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id; END;
