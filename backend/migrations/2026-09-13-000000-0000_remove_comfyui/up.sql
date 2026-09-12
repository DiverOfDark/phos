-- ComfyUI integration removed: workflows, enhancement tasks, production
-- lines, runs and presets all go, together with the columns that recorded a
-- file's generated provenance and a shot's cached description.
--
-- Child tables first — enhancement_tasks references comfyui_workflows and
-- runs; workflow_presets and line_stages reference comfyui_workflows;
-- line_stages and runs reference production_lines.
DROP TABLE IF EXISTS enhancement_tasks;
DROP TABLE IF EXISTS workflow_presets;
DROP TABLE IF EXISTS line_stages;
DROP TABLE IF EXISTS runs;
DROP TABLE IF EXISTS production_lines;
DROP TABLE IF EXISTS comfyui_workflows;

-- The index on files.synthetic must go before the column can.
DROP INDEX IF EXISTS idx_files_synthetic;
ALTER TABLE files DROP COLUMN synthetic;
ALTER TABLE files DROP COLUMN manifest_json;
ALTER TABLE files DROP COLUMN source_workflow_id;
ALTER TABLE files DROP COLUMN source_text_overrides;
ALTER TABLE shots DROP COLUMN analysis_json;
