-- Monthly API call quota tracking.
CREATE TABLE IF NOT EXISTS quota_usage (
  period          TEXT    NOT NULL PRIMARY KEY,  -- 'YYYY-MM'
  calls_total     BIGINT  NOT NULL DEFAULT 0,
  calls_rejected  BIGINT  NOT NULL DEFAULT 0,
  updated_at      TIMESTAMPTZ NOT NULL
);
