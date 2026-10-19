-- Actual child lifecycles; an empty target denotes one shared batch process.
-- Absence on older runs means unknown, never inferred from the aggregate exit status.
CREATE TABLE farm_process_results(
 run_id TEXT NOT NULL REFERENCES farm_runs(id) ON DELETE CASCADE,
 target_id TEXT NOT NULL,
 scope TEXT NOT NULL CHECK(scope IN ('per_target','batch')),
 status TEXT NOT NULL,
 started_at INTEGER NOT NULL,
 finished_at INTEGER,
 exit_code INTEGER,
 PRIMARY KEY(run_id,target_id)
);
