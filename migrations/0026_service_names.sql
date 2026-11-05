-- Core-owned display names do not enter the Proxy's transport configuration.
CREATE TABLE service_names (
 service_id TEXT PRIMARY KEY,
 kind TEXT NOT NULL DEFAULT 'service' CHECK(kind='service'),
 name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 256),
 FOREIGN KEY(kind,service_id) REFERENCES control_attribution(kind,id) ON DELETE CASCADE
);
