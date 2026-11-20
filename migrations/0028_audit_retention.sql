-- Audit stays append-only, but the row cap may evict the oldest records instead of
-- blocking authentication. `floor` is the largest audit id that eviction released;
-- every row above it remains undeletable.
CREATE TABLE IF NOT EXISTS audit_retention(id INTEGER PRIMARY KEY CHECK(id=1), floor INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO audit_retention(id,floor) VALUES(1,0);
DROP TRIGGER IF EXISTS audit_no_delete;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
WHEN old.id > (SELECT floor FROM audit_retention WHERE id=1)
BEGIN SELECT RAISE(ABORT,'append-only audit'); END;
