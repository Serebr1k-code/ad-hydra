-- Deduplicate observation counts per run/target, without retaining another flag value.
CREATE TABLE farm_flag_counter_budget(id INTEGER PRIMARY KEY CHECK(id=1),entries INTEGER NOT NULL DEFAULT 0);
INSERT INTO farm_flag_counter_budget(id) VALUES(1);
CREATE TABLE farm_flag_stats_legacy_runs(run_id TEXT PRIMARY KEY REFERENCES farm_runs(id) ON DELETE CASCADE);
INSERT INTO farm_flag_stats_legacy_runs(run_id) SELECT id FROM farm_runs;
CREATE TABLE farm_flag_observations(run_id TEXT NOT NULL REFERENCES farm_runs(id) ON DELETE CASCADE,target_id TEXT NOT NULL,digest TEXT NOT NULL,is_new INTEGER NOT NULL CHECK(is_new IN (0,1)),PRIMARY KEY(run_id,target_id,digest));
CREATE TRIGGER farm_flag_counts_bound BEFORE INSERT ON farm_flag_observations WHEN (SELECT entries FROM farm_flag_counter_budget WHERE id=1)>=1000000 AND NOT EXISTS(SELECT 1 FROM farm_flag_observations WHERE run_id=NEW.run_id AND target_id=NEW.target_id AND digest=NEW.digest) BEGIN SELECT RAISE(ABORT,'Farm flag observation quota exhausted'); END;
CREATE TRIGGER farm_flag_counts_insert AFTER INSERT ON farm_flag_observations BEGIN UPDATE farm_flag_counter_budget SET entries=entries+1 WHERE id=1; END;
CREATE TRIGGER farm_flag_counts_delete AFTER DELETE ON farm_flag_observations BEGIN UPDATE farm_flag_counter_budget SET entries=entries-1 WHERE id=1; END;
-- Earlier runs can recover known new flags, but their old duplicate counts are unknown.
INSERT OR IGNORE INTO farm_flag_observations(run_id,target_id,digest,is_new) SELECT run_id,COALESCE(target_id,''),digest,1 FROM flags WHERE category='captured' AND run_id IN (SELECT id FROM farm_runs);
