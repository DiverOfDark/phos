-- Lines: a chain of workflows run as one thing a person asked for.
--
-- Phos could already run one workflow against one photo. A *line* is
-- photo → 5s clip → interpolate → 4K upscale as a single request, where the
-- output of each stage is the input of the next. Three tables carry that:
--
--   production_lines  the chain, named
--   line_stages       its steps, ordered, each pinned to a workflow
--   runs              one line applied to one shot
--
-- and `enhancement_tasks` gains the three columns that make a task a step of a
-- run rather than a thing on its own.
--
-- Everything is library-scoped, like the rest of Phos: a line lives in the
-- `.phos.db` beside the photographs it was written for, and travels with them.

CREATE TABLE production_lines (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- One step. `stage_idx` is 0-based and dense; the unique index is what keeps a
-- line linear, and is the thing a reordering editor rewrites.
--
-- The four override columns are the same shapes a single-workflow run already
-- uses, so a stage dispatches through exactly the code an ad-hoc enhance does:
--
--   text_overrides  {"6.text": "..."} plus the `role:<node>` directives the
--                   loader binder reads
--   parameters      {"3.seed": 4242} — typed, one JSON value per field (FR4)
--   vary            {"3.seed": {"count": 4}} — the fan-out spec, expanded once
--                   per continuation so four takes at stage 2 stay four takes
--   source_mode     which part of an upstream video this stage eats:
--                   first_frame / last_frame / at_time:<ms> / keyframe:<n> /
--                   whole_video. NULL lets the graph decide (FR2)
--
-- `keep_output` is the user's choice about the intermediate this stage makes.
-- The default is 0: kept while the run is live so the next stage can read it
-- and a failure can be inspected, discarded when the run completes. The final
-- stage's output is the product and is always kept regardless. FR5c adds a
-- third case — a stage feeding a hold point always keeps — which is why the
-- decision is a function of the stage rather than of this column alone.
CREATE TABLE line_stages (
    id TEXT PRIMARY KEY NOT NULL,
    line_id TEXT NOT NULL REFERENCES production_lines(id),
    stage_idx INTEGER NOT NULL,
    workflow_id TEXT NOT NULL REFERENCES comfyui_workflows(id),
    text_overrides TEXT,
    parameters TEXT,
    vary TEXT,
    source_mode TEXT,
    keep_output BOOLEAN NOT NULL DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_line_stages_position ON line_stages (line_id, stage_idx);

-- One line applied to one shot.
--
-- `line_id` is nullable on purpose: a single-workflow enhance is a one-stage
-- run, so the queue board has one kind of row rather than two and FR7's batch
-- can extend one endpoint rather than two. `label` and `stage_count` are
-- snapshotted at creation so a run still reads correctly after its line is
-- renamed, edited or deleted, and after its finished tasks are swept.
--
-- `status` is derived from the run's tasks and written back here: running while
-- any task is still moving, then failed / cancelled / completed. It is stored
-- rather than computed on every read so the board can page over runs.
CREATE TABLE runs (
    id TEXT PRIMARY KEY NOT NULL,
    line_id TEXT REFERENCES production_lines(id),
    shot_id TEXT NOT NULL REFERENCES shots(id),
    label TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    stage_count INTEGER NOT NULL,
    error_message TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    finished_at TIMESTAMP
);

CREATE INDEX idx_runs_status ON runs (status, created_at);
CREATE INDEX idx_runs_shot ON runs (shot_id);

-- What makes a task a step of a run.
--
--   run_id          which run it belongs to
--   stage_idx       which step of that run's line it is
--   parent_task_id  the task whose output it eats — NULL at stage 0
--
-- The parent link is what makes fan-out propagate: four takes at stage 2 are
-- four rows with four different parents at stage 3, each an independent
-- continuation. It is also the idempotence marker — a completed task has
-- already been advanced exactly when a row names it as parent.
ALTER TABLE enhancement_tasks ADD COLUMN run_id TEXT REFERENCES runs(id);
ALTER TABLE enhancement_tasks ADD COLUMN stage_idx INTEGER;
ALTER TABLE enhancement_tasks ADD COLUMN parent_task_id TEXT REFERENCES enhancement_tasks(id);

CREATE INDEX idx_enhancement_tasks_run ON enhancement_tasks (run_id, stage_idx);
CREATE INDEX idx_enhancement_tasks_parent ON enhancement_tasks (parent_task_id);

-- Give every task that already exists a run, so the board shows one kind of
-- row from the first request after the upgrade rather than hiding the queue
-- somebody left running. Each becomes its own one-stage run; the task's id is
-- reused as the run's, which is unique by construction and readable in a
-- `sqlite3` session.
INSERT INTO runs (id, line_id, shot_id, label, status, stage_count, created_at, finished_at)
SELECT t.id,
       NULL,
       t.shot_id,
       COALESCE(w.name, 'Enhancement'),
       CASE t.status
            WHEN 'completed' THEN 'completed'
            WHEN 'failed'    THEN 'failed'
            WHEN 'cancelled' THEN 'cancelled'
            ELSE 'running'
       END,
       1,
       t.created_at,
       t.completed_at
FROM enhancement_tasks t
LEFT JOIN comfyui_workflows w ON w.id = t.workflow_id;

UPDATE enhancement_tasks SET run_id = id, stage_idx = 0;
