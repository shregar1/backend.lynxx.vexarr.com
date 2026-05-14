-- 013: Analytics, Probes, Snapshots, Alerts, IP Rules, Error Pages, Consumer Groups

-- ── Analytics: time-bucketed request metrics ──

CREATE TABLE IF NOT EXISTS metric_buckets (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id  UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    route_id    UUID,
    bucket_ts   TIMESTAMPTZ NOT NULL,
    granularity TEXT NOT NULL DEFAULT 'minute',
    total_requests  INTEGER NOT NULL DEFAULT 0,
    success_count   INTEGER NOT NULL DEFAULT 0,
    error_4xx       INTEGER NOT NULL DEFAULT 0,
    error_5xx       INTEGER NOT NULL DEFAULT 0,
    total_latency_ms BIGINT NOT NULL DEFAULT 0,
    min_latency_ms   INTEGER,
    max_latency_ms   INTEGER,
    bytes_in    BIGINT NOT NULL DEFAULT 0,
    bytes_out   BIGINT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_metric_buckets_service_ts ON metric_buckets(service_id, bucket_ts DESC);
CREATE INDEX IF NOT EXISTS idx_metric_buckets_granularity ON metric_buckets(granularity, bucket_ts);

CREATE TABLE IF NOT EXISTS geo_buckets (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id   UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    bucket_ts    TIMESTAMPTZ NOT NULL,
    country_code TEXT NOT NULL DEFAULT '',
    city         TEXT NOT NULL DEFAULT '',
    request_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_geo_buckets_service_ts ON geo_buckets(service_id, bucket_ts DESC);

CREATE TABLE IF NOT EXISTS rate_limit_buckets (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id   UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    consumer_id  UUID,
    bucket_ts    TIMESTAMPTZ NOT NULL,
    allowed      INTEGER NOT NULL DEFAULT 0,
    throttled    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_rate_limit_buckets_service_ts ON rate_limit_buckets(service_id, bucket_ts DESC);

-- ── Uptime Probes ──

CREATE TABLE IF NOT EXISTS probes (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id       UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    route_id         UUID,
    name             TEXT NOT NULL,
    method           TEXT NOT NULL DEFAULT 'GET',
    path             TEXT NOT NULL DEFAULT '/',
    interval_seconds INTEGER NOT NULL DEFAULT 60,
    timeout_ms       INTEGER NOT NULL DEFAULT 5000,
    expected_status  INTEGER NOT NULL DEFAULT 200,
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_probes_service ON probes(service_id);

CREATE TABLE IF NOT EXISTS probe_results (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    probe_id   UUID NOT NULL REFERENCES probes(id) ON DELETE CASCADE,
    ts         TIMESTAMPTZ NOT NULL DEFAULT now(),
    status     INTEGER,
    latency_ms INTEGER,
    healthy    BOOLEAN NOT NULL DEFAULT TRUE,
    error      TEXT
);

CREATE INDEX IF NOT EXISTS idx_probe_results_probe_ts ON probe_results(probe_id, ts DESC);

-- ── Snapshots ──

CREATE TABLE IF NOT EXISTS snapshots (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id  UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    data        JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_snapshots_service ON snapshots(service_id, created_at DESC);

-- ── Alert Rules & Events ──

CREATE TABLE IF NOT EXISTS alert_rules (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id       UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    metric           TEXT NOT NULL,
    operator         TEXT NOT NULL DEFAULT 'gt',
    threshold        DOUBLE PRECISION NOT NULL,
    window_seconds   INTEGER NOT NULL DEFAULT 300,
    cooldown_seconds INTEGER NOT NULL DEFAULT 600,
    channels         JSONB NOT NULL DEFAULT '[]',
    enabled          BOOLEAN NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_alert_rules_service ON alert_rules(service_id);

CREATE TABLE IF NOT EXISTS alert_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_id     UUID NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
    service_id  UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    metric      TEXT NOT NULL,
    value       DOUBLE PRECISION NOT NULL,
    threshold   DOUBLE PRECISION NOT NULL,
    message     TEXT NOT NULL DEFAULT '',
    resolved    BOOLEAN NOT NULL DEFAULT FALSE,
    resolved_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_alert_events_service ON alert_events(service_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_alert_events_rule ON alert_events(rule_id, ts DESC);

-- ── IP Rules ──

CREATE TABLE IF NOT EXISTS ip_rules (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id  UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    route_id    UUID,
    cidr        TEXT NOT NULL,
    action      TEXT NOT NULL DEFAULT 'deny',
    description TEXT NOT NULL DEFAULT '',
    priority    INTEGER NOT NULL DEFAULT 0,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    hits        BIGINT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ip_rules_service ON ip_rules(service_id);

-- ── Error Pages ──

CREATE TABLE IF NOT EXISTS error_pages (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id   UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    status_code  INTEGER NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'text/html',
    body         TEXT NOT NULL DEFAULT '',
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_error_pages_service ON error_pages(service_id);

-- ── Consumer Groups ──

CREATE TABLE IF NOT EXISTS consumer_groups (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id          UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    rate_limit          INTEGER NOT NULL DEFAULT 100,
    rate_window_seconds INTEGER NOT NULL DEFAULT 60,
    burst_limit         INTEGER NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_consumer_groups_service ON consumer_groups(service_id);

CREATE TABLE IF NOT EXISTS consumer_group_members (
    group_id    UUID NOT NULL REFERENCES consumer_groups(id) ON DELETE CASCADE,
    consumer_id UUID NOT NULL REFERENCES consumers(id) ON DELETE CASCADE,
    joined_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (group_id, consumer_id)
);
