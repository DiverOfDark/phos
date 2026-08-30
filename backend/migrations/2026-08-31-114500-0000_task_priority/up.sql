-- What a task's hurry is, said out loud.
--
-- Until now the difference between "somebody clicked Enhance and is watching
-- the board" and "the feeder opened this behind three thousand others" was
-- implicit: a task belonged to a run, and the run might or might not carry
-- FR7's `batch_id`. That is a fact about *provenance* being read as a fact
-- about *urgency*, through a join, in the one query that runs every three
-- seconds. Making it a column means the queue says what it means, and the
-- drain order is a single-table sort.
--
-- Two values and no third: `interactive` and `batch`. A scale invites tuning
-- and there is nothing here to tune — the question is only ever "is a person
-- waiting for this".
--
-- The default is `interactive`, which is also what every row written before
-- this migration gets. That is the right way round twice over: every task that
-- exists today was queued a shot at a time by somebody who pressed a button,
-- and an insert that forgets to say fails towards the person rather than
-- towards the farm.
ALTER TABLE enhancement_tasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'interactive';

-- The dispatcher's one query: pending rows, in drain order.
--
-- It carries the whole sort key, so the candidate set is read in something
-- close to the order it is wanted rather than by scanning every task the
-- library has ever run. It does not remove the sort — the priority key is a
-- CASE expression, because two text values that happen to sort correctly today
-- is not a thing to build an ordering on — and it is not claimed to.
CREATE INDEX idx_enhancement_tasks_drain
    ON enhancement_tasks (status, priority, stage_idx, workflow_id, created_at);
