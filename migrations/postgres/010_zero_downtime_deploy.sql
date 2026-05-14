-- Zero-downtime deployment support: target lifecycle state, service
-- drain timeout, and route health gating.

-- Target lifecycle: 'active' | 'draining' | 'inactive'
ALTER TABLE targets ADD COLUMN state TEXT NOT NULL DEFAULT 'active';

-- Per-service drain timeout override (milliseconds). NULL = use gateway default.
ALTER TABLE services ADD COLUMN drain_timeout_ms BIGINT;

-- Route health gate: when true, route only matches if the service has
-- at least one active + healthy target.
ALTER TABLE routes ADD COLUMN health_gate BOOLEAN NOT NULL DEFAULT FALSE;
