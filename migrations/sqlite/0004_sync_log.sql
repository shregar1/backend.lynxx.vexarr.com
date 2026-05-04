-- Cluster sync log used by the SqliteBus polling fallback. Each row
-- represents a ChangeEvent that nodes consume in monotonically increasing
-- `seq` order. Single-node / dev only — Postgres deployments use LISTEN/NOTIFY
-- instead.
CREATE TABLE IF NOT EXISTS _sync_log (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    payload    TEXT    NOT NULL,
    created_at TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sync_log_seq ON _sync_log(seq);
