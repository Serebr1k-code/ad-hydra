ALTER TABLE flag_submitter_config ADD COLUMN mode TEXT NOT NULL DEFAULT 'custom' CHECK(mode IN ('custom','forcad'));
ALTER TABLE flag_submitter_config ADD COLUMN forcad_ip TEXT NOT NULL DEFAULT '';
ALTER TABLE flag_submitter_config ADD COLUMN forcad_port INTEGER NOT NULL DEFAULT 8080 CHECK(forcad_port BETWEEN 1 AND 65535);
ALTER TABLE flag_submitter_config ADD COLUMN team_token TEXT NOT NULL DEFAULT '';
