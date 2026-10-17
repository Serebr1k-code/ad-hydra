CREATE TABLE flag_submitter_config(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL,enabled INTEGER NOT NULL,flag_pattern TEXT NOT NULL,python_packages TEXT NOT NULL,source TEXT NOT NULL,timeout_seconds INTEGER NOT NULL,updated_at INTEGER NOT NULL,author_id TEXT NOT NULL);
CREATE TABLE flag_submitter_clock(id INTEGER PRIMARY KEY CHECK(id=1),next_batch_at INTEGER NOT NULL);
INSERT INTO flag_submitter_clock VALUES(1,CAST(strftime('%s','now') AS INTEGER)+30);
CREATE TABLE flag_submitter_batches(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,flag_count INTEGER NOT NULL,status TEXT NOT NULL,started_at INTEGER NOT NULL,finished_at INTEGER,exit_code INTEGER,stdout_gzip BLOB,stderr_gzip BLOB);
CREATE INDEX flag_submitter_batches_time ON flag_submitter_batches(started_at);
