CREATE TABLE flag_triage (
 flag_id TEXT PRIMARY KEY REFERENCES flags(id), resolution TEXT NOT NULL CHECK(resolution IN ('own_exposed','dismissed','captured')),
 target_id TEXT, actor_id TEXT NOT NULL, request_id TEXT NOT NULL, at INTEGER NOT NULL
);
CREATE INDEX flags_browse ON flags(created_at DESC,id);

ALTER TABLE flag_events ADD COLUMN flow_id TEXT NOT NULL DEFAULT '';
ALTER TABLE flag_events ADD COLUMN service_id TEXT NOT NULL DEFAULT '';
CREATE INDEX flag_events_flow ON flag_events(flow_id);
