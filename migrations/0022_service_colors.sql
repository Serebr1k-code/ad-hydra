CREATE TABLE service_appearance (
    service_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'service' CHECK(kind = 'service'),
    color TEXT NOT NULL CHECK(length(color) = 7),
    FOREIGN KEY(kind, service_id) REFERENCES control_attribution(kind, id) ON DELETE CASCADE
);
