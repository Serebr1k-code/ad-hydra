ALTER TABLE farm_runs ADD COLUMN logs_complete INTEGER NOT NULL DEFAULT 0;
UPDATE farm_runs SET logs_complete=1 WHERE status NOT IN ('queued','running');
ALTER TABLE farm_runs ADD COLUMN stdout_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE farm_runs ADD COLUMN stderr_bytes INTEGER NOT NULL DEFAULT 0;
CREATE TABLE farm_log_chunks (
 id INTEGER PRIMARY KEY, run_id TEXT NOT NULL REFERENCES farm_runs(id) ON DELETE CASCADE,
 stream TEXT NOT NULL CHECK(stream IN ('stdout','stderr')), offset INTEGER NOT NULL,
 raw_bytes INTEGER NOT NULL CHECK(raw_bytes BETWEEN 1 AND 32768), gzip BLOB NOT NULL
);
CREATE INDEX farm_log_chunks_read ON farm_log_chunks(run_id,stream,offset);
CREATE TABLE farm_log_budget(id INTEGER PRIMARY KEY CHECK(id=1), compressed_bytes INTEGER NOT NULL, raw_bytes INTEGER NOT NULL);
INSERT INTO farm_log_budget VALUES(1,0,0);
CREATE TRIGGER farm_log_add AFTER INSERT ON farm_log_chunks BEGIN
 UPDATE farm_log_budget SET compressed_bytes=compressed_bytes+length(NEW.gzip),raw_bytes=raw_bytes+NEW.raw_bytes WHERE id=1;
END;
CREATE TRIGGER farm_log_remove AFTER DELETE ON farm_log_chunks BEGIN
 UPDATE farm_log_budget SET compressed_bytes=compressed_bytes-length(OLD.gzip),raw_bytes=raw_bytes-OLD.raw_bytes WHERE id=1;
END;
-- Include compatibility previews and legacy history in the same compressed budget.
UPDATE farm_log_budget SET compressed_bytes=(SELECT COALESCE(SUM(COALESCE(length(stdout_gzip),length(stdout))+COALESCE(length(stderr_gzip),length(stderr))),0) FROM farm_runs) WHERE id=1;
CREATE TRIGGER farm_log_preview_update AFTER UPDATE OF stdout,stderr,stdout_gzip,stderr_gzip ON farm_runs BEGIN
 UPDATE farm_log_budget SET compressed_bytes=compressed_bytes
  +COALESCE(length(NEW.stdout_gzip),length(NEW.stdout))+COALESCE(length(NEW.stderr_gzip),length(NEW.stderr))
  -COALESCE(length(OLD.stdout_gzip),length(OLD.stdout))-COALESCE(length(OLD.stderr_gzip),length(OLD.stderr)) WHERE id=1;
END;
CREATE TRIGGER farm_log_preview_delete AFTER DELETE ON farm_runs BEGIN
 UPDATE farm_log_budget SET compressed_bytes=compressed_bytes-COALESCE(length(OLD.stdout_gzip),length(OLD.stdout))-COALESCE(length(OLD.stderr_gzip),length(OLD.stderr)) WHERE id=1;
END;
CREATE TRIGGER farm_log_preview_insert AFTER INSERT ON farm_runs BEGIN
 UPDATE farm_log_budget SET compressed_bytes=compressed_bytes+COALESCE(length(NEW.stdout_gzip),length(NEW.stdout))+COALESCE(length(NEW.stderr_gzip),length(NEW.stderr)) WHERE id=1;
END;
