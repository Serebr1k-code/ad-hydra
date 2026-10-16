-- One durable contiguous critical-stream highwater per spool epoch, no flag values.
CREATE TABLE critical_receipts (
    proxy_epoch_id TEXT PRIMARY KEY NOT NULL CHECK(length(proxy_epoch_id) BETWEEN 1 AND 64),
    through_sequence INTEGER NOT NULL CHECK(through_sequence > 0),
    last_event_id TEXT NOT NULL CHECK(length(last_event_id) BETWEEN 1 AND 128)
);
CREATE TRIGGER critical_epoch_bound BEFORE INSERT ON critical_receipts
WHEN NOT EXISTS(SELECT 1 FROM critical_receipts WHERE proxy_epoch_id=NEW.proxy_epoch_id)
AND (SELECT count(*) FROM critical_receipts)>=16
BEGIN SELECT RAISE(ABORT,'critical receipt epoch limit reached'); END;
