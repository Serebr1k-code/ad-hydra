CREATE TABLE traffic_budget(id INTEGER PRIMARY KEY CHECK(id=1),bytes INTEGER NOT NULL);
INSERT INTO traffic_budget SELECT 1, COALESCE((SELECT SUM(length(CAST(data AS BLOB))) FROM traffic_events),0)+COALESCE((SELECT SUM(length(CAST(data AS BLOB))) FROM traffic_messages),0)+COALESCE((SELECT SUM(length(CAST(data AS BLOB))) FROM traffic_flows),0);
CREATE TRIGGER traffic_events_bytes_insert AFTER INSERT ON traffic_events BEGIN UPDATE traffic_budget SET bytes=bytes+length(CAST(NEW.data AS BLOB)); END;
CREATE TRIGGER traffic_events_bytes_delete AFTER DELETE ON traffic_events BEGIN UPDATE traffic_budget SET bytes=bytes-length(CAST(OLD.data AS BLOB)); END;
CREATE TRIGGER traffic_messages_bytes_insert AFTER INSERT ON traffic_messages BEGIN UPDATE traffic_budget SET bytes=bytes+length(CAST(NEW.data AS BLOB)); END;
CREATE TRIGGER traffic_messages_bytes_delete AFTER DELETE ON traffic_messages BEGIN UPDATE traffic_budget SET bytes=bytes-length(CAST(OLD.data AS BLOB)); END;
CREATE TRIGGER traffic_flows_bytes_insert AFTER INSERT ON traffic_flows BEGIN UPDATE traffic_budget SET bytes=bytes+length(CAST(NEW.data AS BLOB)); END;
CREATE TRIGGER traffic_flows_bytes_delete AFTER DELETE ON traffic_flows BEGIN UPDATE traffic_budget SET bytes=bytes-length(CAST(OLD.data AS BLOB)); END;
CREATE TRIGGER traffic_flows_bytes_update AFTER UPDATE OF data ON traffic_flows BEGIN UPDATE traffic_budget SET bytes=bytes+length(CAST(NEW.data AS BLOB))-length(CAST(OLD.data AS BLOB)); END;
