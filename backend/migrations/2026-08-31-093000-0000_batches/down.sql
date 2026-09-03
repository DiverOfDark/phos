-- Rolling back leaves every run a batch opened exactly where it is: a run is a
-- complete thing on its own, and losing the column that says which batch opened
-- it costs the grouping, not the work. Runs that were never opened stay never
-- opened, which is what materialising lazily buys — a rollback mid-batch throws
-- away a cursor, not fifty thousand queued rows.
--
-- The index has to go first: SQLite refuses to drop a column an index names.
DROP INDEX IF EXISTS idx_runs_batch;
ALTER TABLE runs DROP COLUMN batch_id;

DROP INDEX IF EXISTS idx_batches_status;
DROP TABLE batches;
DROP TABLE saved_selections;
