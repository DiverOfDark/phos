-- The old code has no way to apply typed parameters, so a task still waiting to
-- run would silently use the workflow's own defaults for every non-text field.
-- Nothing else is lost: prompts live in `text_overrides`, which is untouched,
-- and a queued task is re-queueable.
ALTER TABLE enhancement_tasks DROP COLUMN parameters;
ALTER TABLE workflow_presets DROP COLUMN parameters;
