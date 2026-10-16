-- Additive optional profile payload; existing WASM rows and activations keep their meaning.
ALTER TABLE protocols ADD COLUMN profile_json TEXT;
ALTER TABLE protocols ADD COLUMN metadata_json TEXT;
