-- Irreversible: the dropped tables held generated-media state (tasks, runs,
-- lines, presets) that no longer has code to read it. Restore the columns so
-- the schema at this point in history is at least shaped right.
ALTER TABLE files ADD COLUMN source_workflow_id TEXT;
ALTER TABLE files ADD COLUMN source_text_overrides TEXT;
ALTER TABLE files ADD COLUMN synthetic BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE files ADD COLUMN manifest_json TEXT;
ALTER TABLE shots ADD COLUMN analysis_json TEXT;
CREATE INDEX IF NOT EXISTS idx_files_synthetic ON files(synthetic);
