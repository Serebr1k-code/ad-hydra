CREATE TABLE traffic_favorites (
 flow_id TEXT PRIMARY KEY NOT NULL,
 raw_history_state TEXT NOT NULL,
 decoded_incomplete INTEGER NOT NULL DEFAULT 0,
 bytes INTEGER NOT NULL DEFAULT 0,
 has_flag INTEGER NOT NULL DEFAULT 0,
 message_count INTEGER NOT NULL DEFAULT 0
);
CREATE TRIGGER favorites_initialize AFTER INSERT ON traffic_favorites BEGIN
 UPDATE traffic_favorites SET has_flag=EXISTS(SELECT 1 FROM flag_events WHERE flow_id=NEW.flow_id),bytes=COALESCE((SELECT sum(length(CAST(data AS BLOB))) FROM traffic_events WHERE flow_id=NEW.flow_id),0)+COALESCE((SELECT sum(length(CAST(data AS BLOB))) FROM traffic_messages WHERE flow_id=NEW.flow_id),0)+COALESCE((SELECT length(CAST(data AS BLOB)) FROM traffic_flows WHERE id=NEW.flow_id),0),message_count=(SELECT count(*) FROM traffic_messages WHERE flow_id=NEW.flow_id) WHERE flow_id=NEW.flow_id;
END;
CREATE TRIGGER favorites_events_insert AFTER INSERT ON traffic_events BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(NEW.data AS BLOB)) WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER favorites_events_delete AFTER DELETE ON traffic_events BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(OLD.data AS BLOB)) WHERE flow_id=OLD.flow_id; END;
CREATE TRIGGER favorites_messages_insert AFTER INSERT ON traffic_messages BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(NEW.data AS BLOB)),message_count=message_count+1 WHERE flow_id=NEW.flow_id; END;
CREATE TRIGGER favorites_messages_delete AFTER DELETE ON traffic_messages BEGIN UPDATE traffic_favorites SET bytes=bytes-length(CAST(OLD.data AS BLOB)),message_count=message_count-1 WHERE flow_id=OLD.flow_id; END;
CREATE TRIGGER favorites_flows_update AFTER UPDATE OF data ON traffic_flows BEGIN UPDATE traffic_favorites SET bytes=bytes+length(CAST(NEW.data AS BLOB))-length(CAST(OLD.data AS BLOB)) WHERE flow_id=NEW.id; END;
CREATE TRIGGER favorites_flags_insert AFTER INSERT ON flag_events BEGIN UPDATE traffic_favorites SET has_flag=1 WHERE flow_id=NEW.flow_id; END;
