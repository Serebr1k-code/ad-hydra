CREATE TABLE flags (
 id TEXT PRIMARY KEY, digest TEXT NOT NULL, category TEXT NOT NULL CHECK(category IN ('captured','own_exposed','unknown')),
 value TEXT NOT NULL, target_id TEXT, run_id TEXT, initiated_by TEXT NOT NULL, created_at INTEGER NOT NULL,
 UNIQUE(digest,category)
);
CREATE TABLE flag_events (event_id TEXT PRIMARY KEY, flag_id TEXT NOT NULL REFERENCES flags(id));
CREATE TABLE submissions (
 id TEXT PRIMARY KEY, flag_id TEXT NOT NULL UNIQUE REFERENCES flags(id), state TEXT NOT NULL DEFAULT 'queued',
 attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL, created_at INTEGER NOT NULL,
 initiated_by TEXT NOT NULL, response TEXT NOT NULL DEFAULT ''
);
CREATE INDEX submissions_due ON submissions(state,next_attempt);
