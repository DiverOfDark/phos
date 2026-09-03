-- Batches: a whole library's worth of shots sent to one line in one action.
--
-- "Everything of Grandma before 1990, restore and upscale, run overnight."
--
-- The row below is the entire batch. It is not fifty thousand run rows waiting
-- to be opened; it is *the query that names them* plus a cursor saying how far
-- the worker has got. The runs are opened a handful at a time, on the same
-- three-second tick that already walks a run along its line.
--
-- Three things fall out of that, and all three are the reason for it:
--
--   * STOP is instant. There is nothing to unwind — the runs that were never
--     opened were never rows.
--   * The queue board stays a board. Fifty thousand `runs` rows would make
--     every read of it a table scan, for work that has not started.
--   * A batch picks up shots imported after it was sent, for free, as long as
--     they sort after its cursor. The query is re-asked every tick; it is not
--     a list frozen at Send.
--
-- Nothing here runs on a timer. A batch exists because a person pressed Send.
-- `window_start_minute`/`window_end_minute` *pace* work that is already queued;
-- they do not start it, and no window at all means "whenever".

CREATE TABLE batches (
    id TEXT PRIMARY KEY NOT NULL,

    -- The line every shot in this batch is sent through, and a human name for
    -- the batch, snapshotted the way `runs.label` is: the batch still reads
    -- correctly after its line is renamed, edited or deleted.
    line_id TEXT NOT NULL,
    label TEXT NOT NULL,

    -- What was selected, as JSON: either `{"kind":"ids","ids":[...]}` or
    -- `{"kind":"query","query":{ ...the shape /api/shots already takes... }}`.
    -- Stored rather than resolved, because resolving it is the whole point:
    -- it is re-asked every tick from the cursor onward.
    selection_json TEXT NOT NULL,

    -- The answers each stage left open, exactly the shape `POST /runs` takes.
    stage_values TEXT,

    -- running | paused | stopped | completed
    --
    -- `paused` is not a state a person puts a batch into. It is what the feeder
    -- writes when a cap says "not now", together with `paused_reason`, and it
    -- goes back to `running` by itself when the cap stops biting. `stopped` is
    -- the one a person causes, and it is terminal.
    status TEXT NOT NULL DEFAULT 'running',
    paused_reason TEXT,

    -- At batch scale, "this shot already has output from this line" is a
    -- *filter*, not the warning dot the Enhance dialog shows for one shot.
    -- Off means redo: run them all again.
    skip_if_generated BOOLEAN NOT NULL DEFAULT 1,

    -- The cursor. A keyset over `(COALESCE(shots.timestamp,''), shots.id)`
    -- ascending, which is a total order because `shots.id` is unique — an
    -- OFFSET would drift under concurrent imports and re-scan what it already
    -- did. NULL/NULL means "not started".
    cursor_key TEXT,
    cursor_shot_id TEXT,

    -- What the confirm sheet said when Send was pressed, kept so the board can
    -- show progress against the number the person agreed to. Estimates, not
    -- promises: the query is re-asked every tick and the library moves.
    matched_total INTEGER,
    skipped_total INTEGER,
    est_tasks INTEGER,
    est_gpu_seconds INTEGER,
    est_disk_bytes BIGINT,

    -- Caps. NULL means "no cap of this kind".
    --
    --   daily_task_cap        tasks this batch may open per calendar day
    --   window_*_minute       minutes from local midnight; may wrap midnight
    --   disk_floor_bytes      pause before the volume fills, not after
    --   max_outstanding_holds the deadlock guard, see below
    --
    -- The outstanding-hold cap is the one that is not obvious. FR5c lets a
    -- stage park its run and ask a person which takes go on. Held runs park
    -- rather than block, so at batch scale 3,329 shots through
    -- `×4 extend → hold → upscale` produce 13,316 clips waiting on a human
    -- before any upscale runs. When more runs of this batch are held than the
    -- cap allows, feeding stops until verdicts bring the number down —
    -- otherwise the farm generates a mountain nobody has looked at.
    daily_task_cap INTEGER,
    window_start_minute INTEGER,
    window_end_minute INTEGER,
    disk_floor_bytes BIGINT,
    max_outstanding_holds INTEGER,

    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    finished_at TIMESTAMP
);

-- The feeder asks one question per tick: which batches are still feeding?
CREATE INDEX idx_batches_status ON batches (status);

-- Which batch opened a run — NULL for every run started one shot at a time,
-- which is every run that exists today.
--
-- Deliberately no REFERENCES clause, matching `files.source_workflow_id`.
-- SQLite refuses `ALTER TABLE ... DROP COLUMN` on a column named in a foreign
-- key, so a REFERENCES here would make this migration's own `down.sql`
-- impossible to run.
ALTER TABLE runs ADD COLUMN batch_id TEXT;

-- Three reads per tick go through this index, and two of them are the caps:
-- how many of this batch's runs are still live, and how many are held.
CREATE INDEX idx_runs_batch ON runs (batch_id, status);

-- A saved selection is a query plus the line you usually send it to. It makes
-- a repeat one click. It never fires on its own — there is no schedule column
-- here and there is not going to be one; a batch exists because a person
-- pressed Send.
CREATE TABLE saved_selections (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    line_id TEXT,
    selection_json TEXT NOT NULL,
    -- The caps this selection is usually sent with, same shape as the columns
    -- above, so pressing Send on it needs no second dialog.
    caps_json TEXT,
    skip_if_generated BOOLEAN NOT NULL DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
