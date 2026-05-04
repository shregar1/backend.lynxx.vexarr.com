CREATE TABLE IF NOT EXISTS targets (
  id TEXT PRIMARY KEY,
  service_id TEXT NOT NULL,
  url TEXT NOT NULL,
  weight INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_targets_service ON targets(service_id);
ALTER TABLE services ADD COLUMN lb_strategy TEXT NOT NULL DEFAULT 'round-robin';
