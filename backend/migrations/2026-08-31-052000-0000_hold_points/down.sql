-- Every hold point becomes an ordinary stage, and every run parked at one goes
-- back to walking its line: the takes it was holding all continue, because
-- without the flag there is nothing to stop them. That is the behaviour this
-- feature replaced, so it is the right thing to fall back to — but a run that
-- was held when the migration ran will spend GPU time on takes a person had not
-- yet chosen, which is worth knowing before running this on a busy library.
UPDATE runs SET status = 'running' WHERE status = 'held';

DROP INDEX IF EXISTS idx_run_holds_run;
DROP TABLE run_holds;
ALTER TABLE runs DROP COLUMN held_at_stage;
ALTER TABLE line_stages DROP COLUMN hold_for_review;
