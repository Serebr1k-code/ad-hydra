CREATE TABLE protocols(id TEXT NOT NULL,version TEXT NOT NULL,sha256 TEXT NOT NULL,state TEXT NOT NULL,created_by TEXT NOT NULL,updated_by TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,error TEXT,PRIMARY KEY(id,version));
CREATE TABLE protocol_operations(actor TEXT NOT NULL,key TEXT NOT NULL,fingerprint TEXT NOT NULL,PRIMARY KEY(actor,key));
CREATE TABLE protocol_activations(service_id TEXT PRIMARY KEY,protocol_id TEXT NOT NULL,version TEXT NOT NULL,applied INTEGER NOT NULL,updated_by TEXT NOT NULL,updated_at INTEGER NOT NULL,error TEXT);
