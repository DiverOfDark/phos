-- Hold points: a stage that parks its run and asks a person which takes go on.
--
-- A line fans out — four seeds, four takes — and today every one of them runs
-- the whole remaining chain. A `×4 extend → upscale 4K` line spends four hours
-- of GPU upscaling three clips nobody was ever going to keep. A hold point puts
-- the choosing *inside* the pipeline: generate the four continuations cheaply,
-- look at them, and only then spend the hour.
--
--   [1]  EXTEND CLIP +5s      ×4 seeds      hold for review
--    |   four candidate continuations land as takes
--   [2]  UPSCALE 4K                         only what you keep pays for this

-- The line's half: which stage asks. Refused on the last stage, because the
-- last stage's output *is* the product and there is nothing after it to hold.
ALTER TABLE line_stages ADD COLUMN hold_for_review BOOLEAN NOT NULL DEFAULT 0;

-- The run's half: where it is parked.
--
-- `runs.status` gains a fifth value, `held`. It is deliberately not terminal —
-- a held run is not over, it is waiting — so nothing sweeps it, nothing settles
-- it, and it survives a restart by being a column rather than a timer. A hold
-- with no verdict stays held: there is no expiry anywhere in this feature.
--
-- `held_at_stage` is the stage whose takes are being looked at. NULL whenever
-- the run is not parked, which is every run that exists today.
ALTER TABLE runs ADD COLUMN held_at_stage INTEGER;

-- The verdicts, append-only.
--
-- One row per verdict, so a run regenerated three times keeps the record of
-- each. Nothing ever updates or deletes one; the history of what a person chose
-- is the point of the feature, not a side effect of it.
--
-- Two arrays rather than one, and the second is what makes the runtime sound:
--
--   reviewed_task_ids  every take this verdict was given over
--   kept_task_ids      the subset that continues (a subset of the above)
--
-- `kept` alone cannot tell a take that was *passed over* from one that has not
-- been looked at yet, and the advance pass has to tell them apart or a run
-- parks again forever on the takes nobody chose. `reviewed` is that marker, and
-- it has exactly the property `enhancement_tasks.parent_task_id` has: it exists
-- precisely when the thing it marks happened, so there is no flag that can
-- disagree with the thing it flags.
--
-- Takes are named by task id rather than by file id. A task *is* a take — it is
-- what `parent_task_id` points at, so continuing from one is the same write the
-- advance pass already makes — and a task id keeps meaning something after its
-- output file has been swept, which a file id does not. A describe stage's
-- takes, which are sentences and have no file at all, are namable for free.
--
--   continue    proceed with `kept_task_ids`; each runs the rest of the line
--   regenerate  run the held stage again with fresh seeds, and hold again
--   cancel      abandon the run
CREATE TABLE run_holds (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id),
    stage_idx INTEGER NOT NULL,
    verdict TEXT NOT NULL,
    reviewed_task_ids TEXT NOT NULL DEFAULT '[]',
    kept_task_ids TEXT NOT NULL DEFAULT '[]',
    note TEXT,
    decided_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- The advance pass asks one question of this table per tick, per run: which of
-- this run's takes has a verdict already been given over?
CREATE INDEX idx_run_holds_run ON run_holds (run_id, stage_idx);
