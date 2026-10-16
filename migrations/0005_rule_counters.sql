CREATE TABLE control_counters(id INTEGER PRIMARY KEY CHECK(id=1),proxy_epoch TEXT NOT NULL,counts TEXT NOT NULL);
INSERT INTO control_counters VALUES(1,'','{}');
