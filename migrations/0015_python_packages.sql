-- Bind runtime and dependency pins to the uploaded immutable exploit version.
ALTER TABLE farm_versions ADD COLUMN runtime_binding TEXT;
UPDATE farm_versions SET runtime_binding=(SELECT json_array(json_extract(manifest,'$.runtime'),json(COALESCE(json_extract(manifest,'$.python_packages'),'[]'))) FROM farm_exploits WHERE id=farm_versions.exploit_id AND version=farm_versions.version);
