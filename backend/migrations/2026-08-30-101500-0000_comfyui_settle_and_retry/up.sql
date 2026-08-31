-- Make a finished ComfyUI run findable, and a hiccup survivable.
--
-- output_prefix   the filename_prefix every save node in the task's workflow was
--                 rewritten to (phos/<task_id>), recorded before the run starts
--                 so the file can be fetched by name when history is empty,
--                 unhelpful, or lost to a ComfyUI restart.
-- settle_until    deadline for the `awaiting_output` state: ComfyUI says the
--                 prompt finished but has published no file yet. Waiting is not
--                 a verdict.
-- next_attempt_at earliest time the worker may touch the row again — the settle
--                 re-check clock, and the backoff between retries of a transient
--                 failure.
ALTER TABLE enhancement_tasks ADD COLUMN output_prefix TEXT;
ALTER TABLE enhancement_tasks ADD COLUMN settle_until DATETIME;
ALTER TABLE enhancement_tasks ADD COLUMN next_attempt_at DATETIME;
