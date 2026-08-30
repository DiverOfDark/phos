-- Say, on the file itself, which pictures a machine made.
--
-- synthetic      the load-bearing flag. Face detection, clustering and primary
--                person assignment all skip a file that carries it, so a
--                generated face can never reach an ArcFace centroid. Until now
--                generated files escaped indexing only because their row
--                already existed when the scanner walked past — an accident,
--                not a rule.
-- manifest_json  how the file was made: the task, the workflow, the prompt
--                overrides, the ComfyUI prompt id, the source file. A versioned
--                object, so later stages of the pipeline can add to it without
--                another migration.
ALTER TABLE files ADD COLUMN synthetic BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE files ADD COLUMN manifest_json TEXT;

-- Every variant a workflow has already produced is synthetic. They have no
-- manifest — nobody recorded one at the time — but the flag is what protects
-- the person model, and that we can still say truthfully.
UPDATE files SET synthetic = 1 WHERE source_workflow_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_files_synthetic ON files(synthetic);
