-- Lines and runs go away entirely. A task queued as a stage of a live line
-- keeps running as the single-workflow task it always was — its source file,
-- prompts and parameters are all on its own row — but nothing will queue the
-- stage after it, so a half-finished chain stops where it is with its
-- intermediates intact on disk.
DROP INDEX IF EXISTS idx_enhancement_tasks_parent;
DROP INDEX IF EXISTS idx_enhancement_tasks_run;
ALTER TABLE enhancement_tasks DROP COLUMN parent_task_id;
ALTER TABLE enhancement_tasks DROP COLUMN stage_idx;
ALTER TABLE enhancement_tasks DROP COLUMN run_id;

DROP INDEX IF EXISTS idx_runs_shot;
DROP INDEX IF EXISTS idx_runs_status;
DROP TABLE IF EXISTS runs;

DROP INDEX IF EXISTS idx_line_stages_position;
DROP TABLE IF EXISTS line_stages;

DROP TABLE IF EXISTS production_lines;
