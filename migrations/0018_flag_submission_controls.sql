-- Keep detection provenance without retaining defensive flag values.
CREATE TABLE flag_events_next(event_id TEXT PRIMARY KEY,flag_id TEXT REFERENCES flags(id),flow_id TEXT NOT NULL DEFAULT '',service_id TEXT NOT NULL DEFAULT '');
-- Move rows incrementally so bounded databases do not need a second full event table.
CREATE TRIGGER move_flag_metadata AFTER DELETE ON flag_events BEGIN
 INSERT INTO flag_events_next(event_id,flag_id,flow_id,service_id) VALUES(OLD.event_id,CASE WHEN EXISTS(SELECT 1 FROM flags WHERE id=OLD.flag_id AND category='own_exposed') THEN NULL ELSE OLD.flag_id END,OLD.flow_id,OLD.service_id);
END;
DELETE FROM flag_events;
DROP TRIGGER move_flag_metadata;
DROP TABLE flag_events;
ALTER TABLE flag_events_next RENAME TO flag_events;
CREATE INDEX flag_events_flow ON flag_events(flow_id);
INSERT INTO audit(at,actor_kind,actor_id,action,target,request_id,success,metadata) SELECT CAST(strftime('%s','now') AS INTEGER),'system','migration-18','flags.purge_defensive','flags','migration-18',1,json_object('removed',count(*)) FROM flags WHERE category='own_exposed';
DELETE FROM submissions WHERE flag_id IN (SELECT id FROM flags WHERE category='own_exposed');
DELETE FROM flag_triage WHERE flag_id IN (SELECT id FROM flags WHERE category='own_exposed');
DELETE FROM flags WHERE category='own_exposed';
CREATE TRIGGER flags_no_defensive_insert BEFORE INSERT ON flags WHEN NEW.category='own_exposed' BEGIN SELECT RAISE(ABORT,'defensive flags are not retained'); END;
CREATE TRIGGER flags_no_defensive_update BEFORE UPDATE OF category ON flags WHEN NEW.category='own_exposed' BEGIN SELECT RAISE(ABORT,'defensive flags are not retained'); END;
ALTER TABLE flags ADD COLUMN service_id TEXT;
CREATE INDEX flags_service_browse ON flags(service_id,created_at DESC,id);
ALTER TABLE flag_submitter_config ADD COLUMN batch_size INTEGER NOT NULL DEFAULT 240 CHECK(batch_size BETWEEN 1 AND 240);
CREATE TABLE flag_submitter_requests(id TEXT PRIMARY KEY,count INTEGER NOT NULL CHECK(count BETWEEN 1 AND 5000),actor_id TEXT NOT NULL,status TEXT NOT NULL,created_at INTEGER NOT NULL,finished_at INTEGER,flag_count INTEGER NOT NULL DEFAULT 0);
CREATE INDEX flag_submitter_requests_state ON flag_submitter_requests(status,created_at);
