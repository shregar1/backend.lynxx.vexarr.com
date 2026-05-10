-- Existing rows were all seeded by --reseed from config.toml in a prior
-- version. Mark them as config-sourced so clear_config_seed handles them.

UPDATE services SET source = 'config' WHERE source = 'api';
UPDATE routes   SET source = 'config' WHERE source = 'api';
UPDATE plugins  SET source = 'config' WHERE source = 'api';
