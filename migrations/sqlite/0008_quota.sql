-- Monthly API call quota tracking.
-- One row per calendar month (YYYY-MM). The gateway reads the current period's
-- row on startup and flushes in-memory counters back every 30 s.
CREATE TABLE IF NOT EXISTS quota_usage (
  period          TEXT    NOT NULL PRIMARY KEY,  -- 'YYYY-MM'
  calls_total     INTEGER NOT NULL DEFAULT 0,
  calls_rejected  INTEGER NOT NULL DEFAULT 0,
  updated_at      TEXT    NOT NULL
);
