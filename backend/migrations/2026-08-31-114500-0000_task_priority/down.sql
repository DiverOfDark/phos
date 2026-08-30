-- Rolling back costs the ordering, not the work: every task keeps its run, its
-- stage and its place in a line, and the queue goes back to draining by
-- `created_at`. A batch mid-flight carries on; it simply interleaves its
-- stages again, which is what it did before FR8.
--
-- The index has to go first: SQLite refuses to drop a column an index names.
DROP INDEX IF EXISTS idx_enhancement_tasks_drain;
ALTER TABLE enhancement_tasks DROP COLUMN priority;
