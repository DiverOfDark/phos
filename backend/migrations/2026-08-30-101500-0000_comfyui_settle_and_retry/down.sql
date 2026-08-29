-- Any task still settling has no state to fall back to, so land it as failed
-- rather than leaving a status the old code does not understand.
UPDATE enhancement_tasks
   SET status = 'failed',
       error_message = COALESCE(error_message, 'Was awaiting output when the schema was rolled back')
 WHERE status = 'awaiting_output';

ALTER TABLE enhancement_tasks DROP COLUMN next_attempt_at;
ALTER TABLE enhancement_tasks DROP COLUMN settle_until;
ALTER TABLE enhancement_tasks DROP COLUMN output_prefix;
