ALTER TABLE services ADD COLUMN document_root TEXT;
ALTER TABLE services ADD COLUMN index_file TEXT NOT NULL DEFAULT 'index.html';
ALTER TABLE services ADD COLUMN spa_fallback INTEGER NOT NULL DEFAULT 0;
