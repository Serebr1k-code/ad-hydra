CREATE TABLE traffic_flows (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, service_id TEXT NOT NULL, occurred_ms INTEGER NOT NULL, client_ip TEXT NOT NULL, data TEXT NOT NULL CHECK(length(CAST(data AS BLOB))<=65536));
CREATE INDEX traffic_flow_service ON traffic_flows(service_id,seq);
CREATE TABLE traffic_messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, flow_id TEXT NOT NULL, service_id TEXT NOT NULL, occurred_ms INTEGER NOT NULL, data TEXT NOT NULL CHECK(length(CAST(data AS BLOB))<=65536));
CREATE INDEX traffic_message_flow ON traffic_messages(flow_id,seq);
CREATE TABLE traffic_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT UNIQUE NOT NULL, flow_id TEXT NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL CHECK(length(CAST(data AS BLOB))<=65536));
