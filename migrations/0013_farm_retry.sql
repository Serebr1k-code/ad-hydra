ALTER TABLE farm_runs ADD COLUMN origin_run_id TEXT REFERENCES farm_runs(id);
CREATE INDEX farm_runs_origin ON farm_runs(origin_run_id);
