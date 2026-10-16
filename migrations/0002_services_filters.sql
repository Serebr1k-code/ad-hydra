CREATE TABLE control_state(id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL, applied_revision INTEGER NOT NULL DEFAULT 0, apply_error TEXT, services_applied_revision INTEGER NOT NULL DEFAULT 0, rules_applied_revision INTEGER NOT NULL DEFAULT 0, services TEXT NOT NULL, rules TEXT NOT NULL);
INSERT INTO control_state (id,revision,applied_revision,apply_error,services,rules) VALUES(1,0,0,NULL,'[]','[]');
CREATE TABLE control_attribution(kind TEXT NOT NULL,id TEXT NOT NULL,created_by TEXT NOT NULL,updated_by TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,hit_count INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(kind,id));
CREATE TABLE rule_tests(id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, tested_at INTEGER NOT NULL);
