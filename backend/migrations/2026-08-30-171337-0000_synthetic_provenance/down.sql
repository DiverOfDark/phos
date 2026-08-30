DROP INDEX IF EXISTS idx_files_synthetic;
ALTER TABLE files DROP COLUMN manifest_json;
ALTER TABLE files DROP COLUMN synthetic;
