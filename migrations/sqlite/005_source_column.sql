-- Track whether a row was seeded from config.toml ('config') or created
-- via the admin API ('api'). seed_from_config only wipes 'config'-sourced
-- rows, so admin-API state survives restarts and --reseed.

ALTER TABLE services ADD COLUMN source TEXT NOT NULL DEFAULT 'api';
ALTER TABLE routes   ADD COLUMN source TEXT NOT NULL DEFAULT 'api';
ALTER TABLE plugins  ADD COLUMN source TEXT NOT NULL DEFAULT 'api';
