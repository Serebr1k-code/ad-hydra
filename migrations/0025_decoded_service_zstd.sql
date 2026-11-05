-- Decoded traffic is zstd level 3, with independent compressed-byte budgets per service.
DROP TRIGGER favorites_initialize;
DROP TRIGGER favorites_events_insert;
DROP TRIGGER favorites_events_delete;
DROP TRIGGER favorites_messages_insert;
DROP TRIGGER favorites_messages_delete;
DROP TRIGGER favorites_flows_update;
DROP TRIGGER traffic_events_bytes_insert;
DROP TRIGGER traffic_events_bytes_delete;
DROP TRIGGER traffic_messages_bytes_insert;
DROP TRIGGER traffic_messages_bytes_delete;
DROP TRIGGER traffic_flows_bytes_insert;
DROP TRIGGER traffic_flows_bytes_delete;
DROP TRIGGER traffic_flows_bytes_update;
DROP INDEX traffic_flow_service;
DROP INDEX traffic_message_flow;
DROP INDEX traffic_flow_time;
DROP INDEX traffic_flow_client;
DROP INDEX traffic_flow_upstream;
DROP INDEX traffic_flow_transport;
DROP INDEX traffic_message_service;
DROP INDEX traffic_message_time;
DROP INDEX traffic_message_direction;
DROP INDEX traffic_rule_lookup;
CREATE TABLE traffic_flows_zstd(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,service_id TEXT NOT NULL,occurred_ms INTEGER NOT NULL,client_ip TEXT NOT NULL,data BLOB NOT NULL CHECK(length(data)<=65792));
INSERT INTO traffic_flows_zstd(seq,id,service_id,occurred_ms,client_ip,data) SELECT seq,id,service_id,occurred_ms,client_ip,hydra_zstd(data) FROM traffic_flows;
DROP TABLE traffic_flows;
CREATE VIEW traffic_flows AS SELECT seq,id,service_id,occurred_ms,client_ip,hydra_text(data) AS data FROM traffic_flows_zstd;
CREATE TRIGGER traffic_flows_insert INSTEAD OF INSERT ON traffic_flows BEGIN INSERT OR IGNORE INTO traffic_flows_zstd(seq,id,service_id,occurred_ms,client_ip,data) VALUES(NEW.seq,NEW.id,NEW.service_id,NEW.occurred_ms,NEW.client_ip,hydra_zstd(NEW.data)); END;
CREATE TRIGGER traffic_flows_delete INSTEAD OF DELETE ON traffic_flows BEGIN DELETE FROM traffic_flows_zstd WHERE seq=OLD.seq; END;
CREATE TRIGGER traffic_flows_update INSTEAD OF UPDATE ON traffic_flows BEGIN UPDATE traffic_flows_zstd SET id=NEW.id,service_id=NEW.service_id,occurred_ms=NEW.occurred_ms,client_ip=NEW.client_ip,data=hydra_zstd(NEW.data) WHERE seq=OLD.seq; END;
CREATE TABLE traffic_messages_zstd(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,flow_id TEXT NOT NULL,service_id TEXT NOT NULL,occurred_ms INTEGER NOT NULL,data BLOB NOT NULL CHECK(length(data)<=65792));
INSERT INTO traffic_messages_zstd(seq,id,flow_id,service_id,occurred_ms,data) SELECT seq,id,flow_id,service_id,occurred_ms,hydra_zstd(data) FROM traffic_messages;
DROP TABLE traffic_messages;
CREATE VIEW traffic_messages AS SELECT seq,id,flow_id,service_id,occurred_ms,hydra_text(data) AS data FROM traffic_messages_zstd;
CREATE TRIGGER traffic_messages_insert INSTEAD OF INSERT ON traffic_messages BEGIN INSERT OR IGNORE INTO traffic_messages_zstd(seq,id,flow_id,service_id,occurred_ms,data) VALUES(NEW.seq,NEW.id,NEW.flow_id,NEW.service_id,NEW.occurred_ms,hydra_zstd(NEW.data)); END;
CREATE TRIGGER traffic_messages_delete INSTEAD OF DELETE ON traffic_messages BEGIN DELETE FROM traffic_messages_zstd WHERE seq=OLD.seq; END;
CREATE TRIGGER traffic_messages_update INSTEAD OF UPDATE ON traffic_messages BEGIN UPDATE traffic_messages_zstd SET id=NEW.id,flow_id=NEW.flow_id,service_id=NEW.service_id,occurred_ms=NEW.occurred_ms,data=hydra_zstd(NEW.data) WHERE seq=OLD.seq; END;
CREATE TABLE traffic_events_zstd(seq INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,flow_id TEXT NOT NULL,kind TEXT NOT NULL,service_id TEXT NOT NULL,data BLOB NOT NULL CHECK(length(data)<=65792));
INSERT INTO traffic_events_zstd(seq,event_id,flow_id,kind,service_id,data) SELECT seq,event_id,flow_id,kind,COALESCE(json_extract(data,'$.service_id'),''),hydra_zstd(data) FROM traffic_events;
DROP TABLE traffic_events;
CREATE VIEW traffic_events AS SELECT seq,event_id,flow_id,kind,service_id,hydra_text(data) AS data FROM traffic_events_zstd;
CREATE TRIGGER traffic_events_insert INSTEAD OF INSERT ON traffic_events BEGIN INSERT OR IGNORE INTO traffic_events_zstd(seq,event_id,flow_id,kind,service_id,data) VALUES(NEW.seq,NEW.event_id,NEW.flow_id,NEW.kind,COALESCE(NEW.service_id,json_extract(NEW.data,'$.service_id'),''),hydra_zstd(NEW.data)); END;
CREATE TRIGGER traffic_events_delete INSTEAD OF DELETE ON traffic_events BEGIN DELETE FROM traffic_events_zstd WHERE seq=OLD.seq; END;
CREATE TRIGGER traffic_events_update INSTEAD OF UPDATE ON traffic_events BEGIN UPDATE traffic_events_zstd SET event_id=NEW.event_id,flow_id=NEW.flow_id,kind=NEW.kind,service_id=NEW.service_id,data=hydra_zstd(NEW.data) WHERE seq=OLD.seq; END;
CREATE INDEX traffic_flow_service ON traffic_flows_zstd(service_id,seq);
CREATE INDEX traffic_flow_time ON traffic_flows_zstd(occurred_ms,seq);
CREATE INDEX traffic_flow_client ON traffic_flows_zstd(client_ip,seq);
CREATE INDEX traffic_flow_upstream ON traffic_flows_zstd(json_extract(hydra_text(data),'$.upstream.ip'),seq);
CREATE INDEX traffic_flow_transport ON traffic_flows_zstd(json_extract(hydra_text(data),'$.transport'),seq);
CREATE INDEX traffic_message_flow ON traffic_messages_zstd(flow_id,seq);
CREATE INDEX traffic_message_service ON traffic_messages_zstd(service_id,seq);
CREATE INDEX traffic_message_time ON traffic_messages_zstd(occurred_ms,seq);
CREATE INDEX traffic_message_direction ON traffic_messages_zstd(flow_id,json_extract(hydra_text(data),'$.direction'),seq);
CREATE INDEX traffic_rule_lookup ON traffic_events_zstd(flow_id,kind,json_extract(hydra_text(data),'$.rule_id'));
CREATE INDEX traffic_event_service ON traffic_events_zstd(service_id,seq);
CREATE INDEX traffic_event_flow ON traffic_events_zstd(flow_id,seq);
CREATE TABLE traffic_service_budget(service_id TEXT PRIMARY KEY NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>=0));
INSERT INTO traffic_service_budget SELECT service_id,sum(bytes) FROM (SELECT service_id,length(data) AS bytes FROM traffic_flows_zstd UNION ALL SELECT service_id,length(data) AS bytes FROM traffic_messages_zstd UNION ALL SELECT service_id,length(data) AS bytes FROM traffic_events_zstd) GROUP BY service_id;
UPDATE traffic_budget SET bytes=COALESCE((SELECT sum(bytes) FROM traffic_service_budget),0) WHERE id=1;
CREATE TRIGGER traffic_flows_zstd_budget_insert AFTER INSERT ON traffic_flows_zstd BEGIN UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_flows_zstd_favorites_insert AFTER INSERT ON traffic_flows_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)) WHERE flow_id=NEW.id; END;
CREATE TRIGGER traffic_flows_zstd_budget_delete AFTER DELETE ON traffic_flows_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id; END;
CREATE TRIGGER traffic_flows_zstd_favorites_delete AFTER DELETE ON traffic_flows_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)) WHERE flow_id=OLD.id; END;
CREATE TRIGGER traffic_flows_zstd_budget_update AFTER UPDATE ON traffic_flows_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id;UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_flows_zstd_favorites_update AFTER UPDATE ON traffic_flows_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)) WHERE flow_id=OLD.id;UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)) WHERE flow_id=NEW.id; END;
CREATE TRIGGER traffic_messages_zstd_budget_insert AFTER INSERT ON traffic_messages_zstd BEGIN UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_messages_zstd_favorites_insert AFTER INSERT ON traffic_messages_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)),message_count=message_count+1 WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER traffic_messages_zstd_budget_delete AFTER DELETE ON traffic_messages_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id; END;
CREATE TRIGGER traffic_messages_zstd_favorites_delete AFTER DELETE ON traffic_messages_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)),message_count=message_count-1 WHERE flow_id=OLD.flow_id; END;
CREATE TRIGGER traffic_messages_zstd_budget_update AFTER UPDATE ON traffic_messages_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id;UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_messages_zstd_favorites_update AFTER UPDATE ON traffic_messages_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)) WHERE flow_id=OLD.flow_id;UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)) WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER traffic_events_zstd_budget_insert AFTER INSERT ON traffic_events_zstd BEGIN UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_events_zstd_favorites_insert AFTER INSERT ON traffic_events_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)) WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER traffic_events_zstd_budget_delete AFTER DELETE ON traffic_events_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id; END;
CREATE TRIGGER traffic_events_zstd_favorites_delete AFTER DELETE ON traffic_events_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)) WHERE flow_id=OLD.flow_id; END;
CREATE TRIGGER traffic_events_zstd_budget_update AFTER UPDATE ON traffic_events_zstd BEGIN UPDATE traffic_budget SET bytes=bytes-length(OLD.data) WHERE id=1;UPDATE traffic_service_budget SET bytes=bytes-length(OLD.data) WHERE service_id=OLD.service_id;UPDATE traffic_budget SET bytes=bytes+length(NEW.data) WHERE id=1;INSERT INTO traffic_service_budget VALUES(NEW.service_id,length(NEW.data)) ON CONFLICT(service_id) DO UPDATE SET bytes=bytes+excluded.bytes; END;
CREATE TRIGGER traffic_events_zstd_favorites_update AFTER UPDATE ON traffic_events_zstd BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(hydra_text(OLD.data) AS BLOB)) WHERE flow_id=OLD.flow_id;UPDATE traffic_favorites SET bytes=bytes+length(CAST(hydra_text(NEW.data) AS BLOB)) WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER favorites_initialize AFTER INSERT ON traffic_favorites BEGIN UPDATE traffic_favorites SET has_flag=EXISTS(SELECT 1 FROM flag_events WHERE flow_id=NEW.flow_id),bytes=COALESCE((SELECT sum(length(CAST(data AS BLOB))) FROM traffic_events WHERE flow_id=NEW.flow_id),0)+COALESCE((SELECT sum(length(CAST(data AS BLOB))) FROM traffic_messages WHERE flow_id=NEW.flow_id),0)+COALESCE((SELECT length(CAST(data AS BLOB)) FROM traffic_flows WHERE id=NEW.flow_id),0),message_count=(SELECT count(*) FROM traffic_messages WHERE flow_id=NEW.flow_id) WHERE flow_id=NEW.flow_id; END;
-- Restore parsed indexes the old independent-table cleanup removed while their
-- complete message events survived. Event sequence preserves original ordering.
-- Move old keys aside first, avoiding transient collisions while remapping.
UPDATE traffic_messages_zstd SET seq=-seq;
UPDATE traffic_messages_zstd SET seq=COALESCE((SELECT e.seq FROM traffic_events_zstd e WHERE e.event_id=json_extract(hydra_text(traffic_messages_zstd.data),'$.event_id')),(SELECT COALESCE(max(seq),0) FROM traffic_events_zstd)-seq);
INSERT OR IGNORE INTO traffic_messages_zstd(seq,id,flow_id,service_id,occurred_ms,data)
SELECT e.seq,json_extract(hydra_text(e.data),'$.message_id'),e.flow_id,e.service_id,COALESCE(json_extract(hydra_text(e.data),'$.occurred_ms'),0),e.data FROM traffic_events_zstd e
WHERE e.kind='message' AND json_type(hydra_text(e.data),'$.message_id')='text'
ORDER BY e.seq;
-- Recover session cards the former independent-table cleanup removed while their
-- messages and opening telemetry were still retained. Never invent missing bytes.
INSERT OR IGNORE INTO traffic_flows_zstd(id,service_id,occurred_ms,client_ip,data)
SELECT e.flow_id,e.service_id,COALESCE(json_extract(e.data,'$.occurred_ms'),0),COALESCE(json_extract(e.data,'$.client.ip'),''),hydra_zstd(json_set(e.data,'$.closed',json(COALESCE((SELECT c.data FROM traffic_events c WHERE c.flow_id=e.flow_id AND c.kind='flow_closed' ORDER BY c.seq DESC LIMIT 1),'null'))))
FROM traffic_events e WHERE e.kind='flow_opened'
AND NOT EXISTS(SELECT 1 FROM traffic_flows_zstd f WHERE f.id=e.flow_id)
AND EXISTS(SELECT 1 FROM traffic_messages_zstd m WHERE m.flow_id=e.flow_id)
ORDER BY e.seq;
