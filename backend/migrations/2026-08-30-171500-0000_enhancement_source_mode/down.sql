-- The old code reads every video source as frame zero, so a task still waiting
-- to run would silently do the wrong thing. Nothing is lost by dropping the
-- column: a queued task is re-queueable, and a finished one has its output.
ALTER TABLE enhancement_tasks DROP COLUMN source_mode;
