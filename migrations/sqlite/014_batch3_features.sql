-- Batch 3: cache, tracing, transforms, versioning, incidents, webhooks,
-- load testing, marketplace, cost analytics, cluster, rate-limit simulator, PKI

CREATE TABLE IF NOT EXISTS cache_entries (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    cache_key       TEXT NOT NULL,
    content_hash    TEXT NOT NULL DEFAULT '',
    size_bytes      INTEGER NOT NULL DEFAULT 0,
    ttl_seconds     INTEGER NOT NULL DEFAULT 300,
    hits            INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    expires_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_cache_entries_service ON cache_entries(service_id) WHERE is_deleted = 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_cache_entries_key ON cache_entries(service_id, cache_key) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS traces (
    id              TEXT PRIMARY KEY,
    trace_id        TEXT NOT NULL,
    service_id      TEXT NOT NULL,
    route_id        TEXT,
    method          TEXT NOT NULL DEFAULT 'GET',
    path            TEXT NOT NULL DEFAULT '/',
    status          INTEGER NOT NULL DEFAULT 200,
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    span_count      INTEGER NOT NULL DEFAULT 0,
    has_errors      INTEGER NOT NULL DEFAULT 0,
    metadata        TEXT,
    started_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_traces_service ON traces(service_id, started_at) WHERE is_deleted = 0;
CREATE INDEX IF NOT EXISTS idx_traces_trace_id ON traces(trace_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS spans (
    id              TEXT PRIMARY KEY,
    trace_id        TEXT NOT NULL,
    span_id         TEXT NOT NULL,
    parent_span_id  TEXT,
    operation       TEXT NOT NULL DEFAULT '',
    service_name    TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'ok',
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    tags            TEXT,
    logs            TEXT,
    started_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_spans_trace ON spans(trace_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS transform_rules (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    route_id        TEXT,
    name            TEXT NOT NULL DEFAULT '',
    phase           TEXT NOT NULL DEFAULT 'request',
    action          TEXT NOT NULL DEFAULT 'set_header',
    target          TEXT NOT NULL DEFAULT '',
    value           TEXT NOT NULL DEFAULT '',
    condition       TEXT,
    priority        INTEGER NOT NULL DEFAULT 0,
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_transform_rules_service ON transform_rules(service_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS api_versions (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    version         TEXT NOT NULL DEFAULT '',
    base_path       TEXT NOT NULL DEFAULT '',
    upstream_path   TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    deprecated_at   TEXT,
    sunset_at       TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_api_versions_service ON api_versions(service_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS incidents (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    title           TEXT NOT NULL DEFAULT '',
    severity        TEXT NOT NULL DEFAULT 'minor',
    status          TEXT NOT NULL DEFAULT 'investigating',
    description     TEXT NOT NULL DEFAULT '',
    started_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    resolved_at     TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_incidents_service ON incidents(service_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS incident_updates (
    id              TEXT PRIMARY KEY,
    incident_id     TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT '',
    message         TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (incident_id) REFERENCES incidents(id)
);
CREATE INDEX IF NOT EXISTS idx_incident_updates_incident ON incident_updates(incident_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS webhooks (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    name            TEXT NOT NULL DEFAULT '',
    url             TEXT NOT NULL DEFAULT '',
    events          TEXT NOT NULL DEFAULT '[]',
    secret          TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    retry_count     INTEGER NOT NULL DEFAULT 3,
    timeout_ms      INTEGER NOT NULL DEFAULT 5000,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_webhooks_service ON webhooks(service_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id              TEXT PRIMARY KEY,
    webhook_id      TEXT NOT NULL,
    event_type      TEXT NOT NULL DEFAULT '',
    payload         TEXT NOT NULL DEFAULT '{}',
    response_status INTEGER,
    response_body   TEXT,
    success         INTEGER NOT NULL DEFAULT 0,
    attempts        INTEGER NOT NULL DEFAULT 1,
    delivered_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (webhook_id) REFERENCES webhooks(id)
);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook ON webhook_deliveries(webhook_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS load_tests (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    name            TEXT NOT NULL DEFAULT '',
    target_rps      INTEGER NOT NULL DEFAULT 100,
    duration_seconds INTEGER NOT NULL DEFAULT 30,
    concurrent_users INTEGER NOT NULL DEFAULT 10,
    method          TEXT NOT NULL DEFAULT 'GET',
    path            TEXT NOT NULL DEFAULT '/',
    headers         TEXT,
    body            TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    result          TEXT,
    started_at      TEXT,
    completed_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_load_tests_service ON load_tests(service_id) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS marketplace_plugins (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL DEFAULT '',
    description     TEXT NOT NULL DEFAULT '',
    version         TEXT NOT NULL DEFAULT '0.1.0',
    author          TEXT NOT NULL DEFAULT '',
    category        TEXT NOT NULL DEFAULT 'general',
    schema          TEXT NOT NULL DEFAULT '{}',
    readme          TEXT NOT NULL DEFAULT '',
    downloads       INTEGER NOT NULL DEFAULT 0,
    rating          REAL NOT NULL DEFAULT 0.0,
    verified        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS cost_records (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    bucket_ts       TEXT NOT NULL,
    compute_cost    REAL NOT NULL DEFAULT 0.0,
    bandwidth_cost  REAL NOT NULL DEFAULT 0.0,
    storage_cost    REAL NOT NULL DEFAULT 0.0,
    total_cost      REAL NOT NULL DEFAULT 0.0,
    currency        TEXT NOT NULL DEFAULT 'USD',
    requests        INTEGER NOT NULL DEFAULT 0,
    bytes_transferred INTEGER NOT NULL DEFAULT 0,
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_cost_records_service ON cost_records(service_id, bucket_ts) WHERE is_deleted = 0;

CREATE TABLE IF NOT EXISTS cluster_nodes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL DEFAULT '',
    address         TEXT NOT NULL DEFAULT '',
    port            INTEGER NOT NULL DEFAULT 0,
    role            TEXT NOT NULL DEFAULT 'worker',
    status          TEXT NOT NULL DEFAULT 'active',
    version         TEXT NOT NULL DEFAULT '',
    cpu_usage       REAL NOT NULL DEFAULT 0.0,
    memory_usage    REAL NOT NULL DEFAULT 0.0,
    active_connections INTEGER NOT NULL DEFAULT 0,
    last_heartbeat  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    joined_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS rate_limit_simulations (
    id              TEXT PRIMARY KEY,
    service_id      TEXT NOT NULL,
    consumer_id     TEXT,
    requests_per_second INTEGER NOT NULL DEFAULT 100,
    duration_seconds INTEGER NOT NULL DEFAULT 10,
    rate_limit      INTEGER NOT NULL DEFAULT 60,
    window_seconds  INTEGER NOT NULL DEFAULT 60,
    burst           INTEGER NOT NULL DEFAULT 10,
    result          TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);

CREATE TABLE IF NOT EXISTS certificates (
    id              TEXT PRIMARY KEY,
    service_id      TEXT,
    domain          TEXT NOT NULL DEFAULT '',
    issuer          TEXT NOT NULL DEFAULT '',
    serial_number   TEXT NOT NULL DEFAULT '',
    not_before      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    not_after       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    fingerprint_sha256 TEXT NOT NULL DEFAULT '',
    auto_renew      INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'active',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    is_deleted      INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (service_id) REFERENCES services(id)
);
CREATE INDEX IF NOT EXISTS idx_certificates_domain ON certificates(domain) WHERE is_deleted = 0;
