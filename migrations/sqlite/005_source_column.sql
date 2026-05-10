-- Track whether a row was seeded from config.toml ('config') or created
-- via the admin API ('api'). seed_from_config only wipes 'config'-sourced
-- rows, so admin-API state survives restarts and --reseed.
--
-- Default is 'api' so admin-API-created rows are tagged correctly going
-- forward. Existing rows are then marked 'config' because they were all
-- inserted by --reseed from config.toml in a prior version.

ALTER TABLE services ADD COLUMN source TEXT NOT NULL DEFAULT 'api';
ALTER TABLE routes   ADD COLUMN source TEXT NOT NULL DEFAULT 'api';
ALTER TABLE plugins  ADD COLUMN source TEXT NOT NULL DEFAULT 'api';

UPDATE services SET source = 'config';
UPDATE routes   SET source = 'config';
UPDATE plugins  SET source = 'config';
