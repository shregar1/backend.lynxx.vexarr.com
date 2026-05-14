-- Batch 3: cache, tracing, transforms, versioning, incidents, webhooks,
-- load testing, marketplace, cost analytics, cluster, rate-limit simulator, PKI

CREATE TABLE IF NOT EXISTS cache_entries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    cache_key       TEXT NOT NULL,
    content_hash    TEXT NOT NULL DEFAULT '',
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    ttl_seconds     INTEGER NOT NULL DEFAULT 300,
    hits            BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_cache_entries_service ON cache_entries(service_id) WHERE NOT is_deleted;
CREATE UNIQUE INDEX IF NOT EXISTS idx_cache_entries_key ON cache_entries(service_id, cache_key) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS traces (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    trace_id        TEXT NOT NULL,
    service_id      UUID NOT NULL REFERENCES services(id),
    route_id        UUID,
    method          TEXT NOT NULL DEFAULT 'GET',
    path            TEXT NOT NULL DEFAULT '/',
    status          INTEGER NOT NULL DEFAULT 200,
    duration_ms     BIGINT NOT NULL DEFAULT 0,
    span_count      INTEGER NOT NULL DEFAULT 0,
    has_errors      BOOLEAN NOT NULL DEFAULT false,
    metadata        JSONB,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_traces_service ON traces(service_id, started_at) WHERE NOT is_deleted;
CREATE INDEX IF NOT EXISTS idx_traces_trace_id ON traces(trace_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS spans (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    trace_id        TEXT NOT NULL,
    span_id         TEXT NOT NULL,
    parent_span_id  TEXT,
    operation       TEXT NOT NULL DEFAULT '',
    service_name    TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'ok',
    duration_ms     BIGINT NOT NULL DEFAULT 0,
    tags            JSONB,
    logs            JSONB,
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_spans_trace ON spans(trace_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS transform_rules (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    route_id        UUID,
    name            TEXT NOT NULL DEFAULT '',
    phase           TEXT NOT NULL DEFAULT 'request',
    action          TEXT NOT NULL DEFAULT 'set_header',
    target          TEXT NOT NULL DEFAULT '',
    value           TEXT NOT NULL DEFAULT '',
    condition       TEXT,
    priority        INTEGER NOT NULL DEFAULT 0,
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_transform_rules_service ON transform_rules(service_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS api_versions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    version         TEXT NOT NULL DEFAULT '',
    base_path       TEXT NOT NULL DEFAULT '',
    upstream_path   TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    deprecated_at   TIMESTAMPTZ,
    sunset_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_api_versions_service ON api_versions(service_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS incidents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    title           TEXT NOT NULL DEFAULT '',
    severity        TEXT NOT NULL DEFAULT 'minor',
    status          TEXT NOT NULL DEFAULT 'investigating',
    description     TEXT NOT NULL DEFAULT '',
    started_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_incidents_service ON incidents(service_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS incident_updates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id     UUID NOT NULL REFERENCES incidents(id),
    status          TEXT NOT NULL DEFAULT '',
    message         TEXT NOT NULL DEFAULT '',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_incident_updates_incident ON incident_updates(incident_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS webhooks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    name            TEXT NOT NULL DEFAULT '',
    url             TEXT NOT NULL DEFAULT '',
    events          JSONB NOT NULL DEFAULT '[]'::jsonb,
    secret          TEXT,
    enabled         BOOLEAN NOT NULL DEFAULT true,
    retry_count     INTEGER NOT NULL DEFAULT 3,
    timeout_ms      INTEGER NOT NULL DEFAULT 5000,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_webhooks_service ON webhooks(service_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    webhook_id      UUID NOT NULL REFERENCES webhooks(id),
    event_type      TEXT NOT NULL DEFAULT '',
    payload         JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_status INTEGER,
    response_body   TEXT,
    success         BOOLEAN NOT NULL DEFAULT false,
    attempts        INTEGER NOT NULL DEFAULT 1,
    delivered_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook ON webhook_deliveries(webhook_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS load_tests (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    name            TEXT NOT NULL DEFAULT '',
    target_rps      INTEGER NOT NULL DEFAULT 100,
    duration_seconds INTEGER NOT NULL DEFAULT 30,
    concurrent_users INTEGER NOT NULL DEFAULT 10,
    method          TEXT NOT NULL DEFAULT 'GET',
    path            TEXT NOT NULL DEFAULT '/',
    headers         JSONB,
    body            TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    result          JSONB,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_load_tests_service ON load_tests(service_id) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS marketplace_plugins (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL DEFAULT '',
    description     TEXT NOT NULL DEFAULT '',
    version         TEXT NOT NULL DEFAULT '0.1.0',
    author          TEXT NOT NULL DEFAULT '',
    category        TEXT NOT NULL DEFAULT 'general',
    schema          JSONB NOT NULL DEFAULT '{}'::jsonb,
    readme          TEXT NOT NULL DEFAULT '',
    downloads       BIGINT NOT NULL DEFAULT 0,
    rating          DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    verified        BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE IF NOT EXISTS cost_records (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    bucket_ts       TIMESTAMPTZ NOT NULL,
    compute_cost    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    bandwidth_cost  DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    storage_cost    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    total_cost      DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    currency        TEXT NOT NULL DEFAULT 'USD',
    requests        BIGINT NOT NULL DEFAULT 0,
    bytes_transferred BIGINT NOT NULL DEFAULT 0,
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_cost_records_service ON cost_records(service_id, bucket_ts) WHERE NOT is_deleted;

CREATE TABLE IF NOT EXISTS cluster_nodes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL DEFAULT '',
    address         TEXT NOT NULL DEFAULT '',
    port            INTEGER NOT NULL DEFAULT 0,
    role            TEXT NOT NULL DEFAULT 'worker',
    status          TEXT NOT NULL DEFAULT 'active',
    version         TEXT NOT NULL DEFAULT '',
    cpu_usage       DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    memory_usage    DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    active_connections BIGINT NOT NULL DEFAULT 0,
    last_heartbeat  TIMESTAMPTZ NOT NULL DEFAULT now(),
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE IF NOT EXISTS rate_limit_simulations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID NOT NULL REFERENCES services(id),
    consumer_id     UUID,
    requests_per_second INTEGER NOT NULL DEFAULT 100,
    duration_seconds INTEGER NOT NULL DEFAULT 10,
    rate_limit      INTEGER NOT NULL DEFAULT 60,
    window_seconds  INTEGER NOT NULL DEFAULT 60,
    burst           INTEGER NOT NULL DEFAULT 10,
    result          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE IF NOT EXISTS certificates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      UUID REFERENCES services(id),
    domain          TEXT NOT NULL DEFAULT '',
    issuer          TEXT NOT NULL DEFAULT '',
    serial_number   TEXT NOT NULL DEFAULT '',
    not_before      TIMESTAMPTZ NOT NULL DEFAULT now(),
    not_after       TIMESTAMPTZ NOT NULL DEFAULT now(),
    fingerprint_sha256 TEXT NOT NULL DEFAULT '',
    auto_renew      BOOLEAN NOT NULL DEFAULT true,
    status          TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_deleted      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS idx_certificates_domain ON certificates(domain) WHERE NOT is_deleted;
