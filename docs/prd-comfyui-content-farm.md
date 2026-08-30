# PRD: ComfyUI Content Farm

## Status
Draft

## Author
Kirill Orlov (drafted with Claude Code)

## Date
2026-08-30

## Summary

Phos already knows **who and what is in every photo** — faces clustered to named people, a
Florence-2 caption on every shot, EXIF time and place, originals distinguished from variants.
No other ComfyUI frontend has that. Today the knowledge stops at a modal dialog where you pick
one workflow, retype a prompt by hand, and run it against one photo.

This PRD turns that dialog into a **production line**: pre-built templates that work on first
install, whole-library batches driven by a query rather than a list of IDs, chained multi-stage
generation (photo → clip → extend → 4K), prompts compiled from what Phos already knows, and a
curation lane — sitting on a reliability floor, because a farm on a flaky pipe multiplies the
flakiness by ten thousand.

Three shifts:

1. From **one shot → one workflow** to **a query → a line**.
2. From **hand-typed prompts** to **prompts compiled from library knowledge**.
3. From **"did it work?"** to **"which of these takes do I keep?"**

---

## Why this exists

The current integration was built to answer "can Phos call ComfyUI at all?" It answers that well.
It does not answer "can Phos produce content at volume without supervision?", which is the actual
goal:

- **Edited photos** — restore, upscale, retouch at library scale.
- **Video generated from photos** — image-to-video on shots Phos already knows are worth it.
- **Extended video** — take an existing clip and continue it.
- **Quality tiers** — 720p to 4K, 24fps to 60fps, as a routine pass rather than a project.

Every one of those is either impossible or a manual slog today, and the reasons are structural,
not cosmetic.

---

## Repository findings (current `master`)

This PRD is grounded in the code as of commit `0e16e36`.

### What already works

- **`ComfyUiClient`** (`backend/src/comfyui.rs`) — health check, multipart image upload,
  `queue_prompt`, `get_history`, `is_prompt_in_queue`, `download_output`.
- **Workflow analysis** — `detect_inputs` surfaces `LoadImage.image`, `CLIPTextEncode.text`, and
  string widgets on classes whose name contains `String`/`Text`. `detect_outputs` recognises
  `SaveImage`, `PreviewImage`, `VHS_VideoCombine`, `SaveAnimatedWEBP`, `SaveAnimatedPNG`.
- **Background worker** — `spawn_enhancement_worker` polls on a 3s tick: claim up to 5 pending
  tasks, upload source, queue prompt, poll history, download outputs, clean up completed tasks
  older than 5 minutes. Per-library, spawned per user in multi-user mode.
- **Persistence** — outputs are written next to the original as
  `{stem}_enhanced_{task8}.{ext}` and inserted into `files` with `is_original = false`,
  `source_workflow_id` and `source_text_overrides`. Provenance is already partly modelled.
- **Tables** — `comfyui_workflows`, `enhancement_tasks`, `workflow_presets`.
- **API** — 15 routes under `/api/comfyui/` (health, workflows, graph, presets, enhance, tasks,
  generations).
- **UI** — `WorkflowsPage.vue` (health, import, preset editor, live queue),
  `EnhanceDialog.vue` (workflow → preset → text overrides → run),
  `WorkflowGraph.vue` (renders a workflow as a derived route diagram, since the API-format JSON
  keeps no geometry).
- **Captioning** — Florence-2-large-ft ONNX, lazily loaded and unloaded (~750 MB), writing a
  paragraph into `shots.description`, already searchable through `/api/shots`.

### Structural gaps

| # | Gap | Evidence |
|---|---|---|
| G1 | **Video enters as a single first frame.** No video→video path at all. | `get_source_image`, `comfyui.rs:346-395` |
| G2 | **No chaining.** Every task is an island; nothing consumes an output. | `enhancement_tasks` has no run/stage/parent columns |
| G3 | **Only strings are overridable.** No seed, steps, cfg, frame count, resolution, LoRA. | `detect_inputs`, `comfyui.rs:200-255` |
| G4 | **Every `LoadImage` node gets the same file.** Start-frame/end-frame workflows are impossible. | `prepare_workflow`, `comfyui.rs:283-291` |
| G5 | **One shot per request.** No batch, no query, no cadence. | `EnhancePayload`, `api/comfyui.rs:295` |
| G6 | **`/object_info` is never called.** Node types are guessed by sniffing class-name substrings. | no occurrences in repo |
| G7 | **`_meta.title` is discarded on import.** The author's own node names are thrown away. | no occurrences in `api/comfyui.rs` |
| G8 | **No scheduling.** No priority, concurrency limit, budget, quiet hours, or VRAM awareness. | `process_pending_tasks` limit(5) per tick |
| G9 | **No curation surface.** Generated files land as ordinary variants; nothing rates or rejects. | Review Desk has three lanes, none for takes |
| G10 | **Completion detection is fragile.** Successful ComfyUI runs are reported as failures. | `comfyui.rs:706-742`, `:833-880` |

### Quality notes

- `backend/src/comfyui.rs` has **no tests** — 1017 lines, zero coverage, and it is the component
  users report as flaky.
- `retry_count` is read in six places and reset on manual retry, but **never incremented**.
  Automatic retry was deliberately removed in `857e055`.
- `WorkflowsPage.vue:312` calls `POST /api/comfyui/tasks/{id}/cancel`; **no such route exists**.
  The cancel button silently 404s.
- Generated files receive no `visual_embedding` and no face rows — but only *incidentally*,
  because the worker inserts into `files` directly and bypasses analysis.

---

## Product goals

### Primary goals

1. **Send a library-scale selection to a multi-stage line in one action.** Tens of thousands of
   shots, one confirm, walk away.
2. **Ship templates that work on first install**, and report honestly when the ComfyUI box is
   missing the nodes or models a template needs.
3. **Never lose a completed ComfyUI job.** A task marked failed must be a task that actually
   failed.
4. **Make curation, not generation, where the time goes.** Generation is cheap; deciding is not.
5. **Keep synthetic media distinguishable from real photographs, permanently.** In a family
   archive, ten years on, this is not a compliance checkbox.

### Secondary goals

- Prompts authored from library knowledge (people, caption, place, date) rather than by hand.
- Full reproducibility: any output can be re-made, or varied, from its recorded manifest.
- Cost visibility — estimated GPU hours and disk before a batch is committed.

### Non-goals (v1)

- **Building or editing workflows inside Phos.** ComfyUI's editor stays the editor; Phos imports
  API-format JSON and runs it.
- **Branching lines.** v1 lines are linear, with fan-out and hold points covering the known cases.
  Branching would need a real graph editor, which is the thing this PRD deliberately leaves to
  ComfyUI.
- **Android support for Takes.** Review already works on the phone, but take curation stays
  desktop-only for v1. Accepted consequence: a held run waits until someone is at a desk.
- **Anything on a timer.** No cron, no scheduled jobs, no standing orders — every batch is started
  by a person. Saved selections make a repeat one click; they never fire on their own.
- Multi-GPU or multi-host ComfyUI farming.
- Training, fine-tuning, or LoRA creation.
- Cloud or hosted generation, and publishing to social platforms.

---

## Target users

**Primary:** a self-hoster with a GPU box and a family media library synced over Nextcloud.
Reviews on desktop, browses on the phone, is comfortable installing ComfyUI custom nodes but does
not want to babysit a queue.

**Secondary:** the same person six months later, looking at a video in their library and asking
"how did I make this, and can I make another?"

---

## User stories

1. **Make this photo move.** "I open a shot, press one button, and a few minutes later there is a
   five-second clip attached to it — and I never typed a prompt."
2. **Restore the box of scans.** "Everything of Grandma before 1990, restore and upscale, run
   overnight. In the morning I go through what came out."
3. **Four takes.** "Same photo, same line, four different seeds. Show me all four side by side and
   let me keep one."
4. **Extend the clip.** "This five-second clip should be fifteen. Continue it from its last
   frame."
5. **Year in review.** "I pick this year's best shots of each person, send the whole set through the
   reel line in one go, and come back to it tomorrow."
6. **Trust the queue.** "When Phos says failed, I want it to mean failed — not 'ComfyUI finished
   and Phos looked in the wrong place'."

---

## UX requirements

### The Depot (replaces the Workflows page)

Four sections, in the existing AppBahn register — 1px borders, uppercase mono for labels and ids,
status colour as the only non-amber colour:

- **Templates** — the bundled set. Each row shows readiness: `READY`, `MISSING 2 NODES`,
  `MISSING MODEL wan2.1_i2v_720p.safetensors`, with exact install names.
- **Stages** — imported workflows (what the page shows today), with the derived route diagram.
- **Lines** — ordered stages, with the editor described in FR5b. Read-only, a line renders with the
  same diagram vocabulary as `WorkflowGraph.vue`, because a line is the same kind of fact as a
  workflow: nodes and the links between them. Under edit it is a vertical list, because that is
  what you can actually manipulate without re-implementing a canvas.
- **Saved selections** — a query plus the line you usually send it to, so a repeat batch is one
  click rather than a rebuilt filter. Saved, not scheduled: it waits for you to press Send.

### Send to line

Reachable from three places, all producing the same confirm sheet:

- a single shot (today's Enhance dialog, retargeted at lines)
- a multi-selection in the gallery
- a saved query — **the primary path at scale**

The confirm sheet is the guardrail. Before anything is queued:

```
12,431 shots matched
 9,102 already have output from this line          [skip]  [redo]
 3,329 to run  ·  ×2 seeds  =  6,658 tasks
 est. 41 h GPU  ·  est. 780 GB disk
 window: 00:00–07:00  ·  cap: 400 tasks/day
                                        [ Cancel ]  [ Send ]
```

### The board (queue)

One row per **run**, not per task. `SHOT 04182 · 4K RESTORE · STAGE 2/4 · UPSCALE · 00:03:12`.
Batch rows collapse into a single progress row with a live count and a STOP that also purges
ComfyUI's own queue.

### Takes (fourth Review Desk lane)

A contact sheet of generated output awaiting a verdict, keyboard-first, matching the muscle memory
the other three lanes already teach:

- `1`–`5` rate · `X` reject · `Enter` keep · `space` play/pause video · `P` promote to main file
- Reject **deletes the bytes** — generated video is enormous and a farm fills a disk in days.
- Compare mode: original and takes side by side, or a 2×2 grid for a seed fan-out.

### Provenance panel

On any generated file: the line, the stage, the parent file, the seed, the compiled prompt, the
model checkpoint, and the ComfyUI prompt id — with **[Make another like this]** and
**[Edit and re-run]**.

---

## Functional requirements

### FR1. Reliability floor

*Prerequisite for everything else. In flight as a separate PR.*

- Rewrite output nodes' `filename_prefix` to `phos/{task_id}` so the output filename is known
  before the run starts, recoverable after a ComfyUI restart, and garbage-collectable.
- Scan **every** key in the history `outputs` object for arrays of objects carrying a `filename`,
  rather than reading only `images` and `gifs`. The same refusal to hard-code keys extends to
  inline values, which FR9's describe stage depends on.
- Replace "5 retries × 2s then permanent failure" with an `awaiting_output` state and a real
  budget — ~60s for image workflows, ~15 min when the workflow has a video output node.
- Split transient failures (upload, queue, history, download IO) from permanent ones (prompt
  validation, node exception). Retry the former with backoff using the existing `retry_count`
  column; fail the latter immediately with the real traceback.
- Surface the actual download error instead of the generic "No output images found".
- Add the missing `POST /api/comfyui/tasks/{id}/cancel`, interrupting the running prompt and
  removing it from ComfyUI's queue.
- Send `client_id` on `/prompt` (prerequisite for FR1b).

**FR1b (follow-up, not v1):** subscribe to ComfyUI's `/ws` and treat `executed` /
`execution_error` as the authoritative completion signal, demoting history polling to a fallback.

**Acceptance:** table-driven tests over realistic history payloads — outputs under `images`,
`gifs`, `videos`, an unknown custom key, `{}`, `null`, an `execution_error`, and a cached
execution. Each test must fail against the pre-fix code and pass after.

### FR2. Video in, video out

- Upload accepts video mime types (ComfyUI's `/upload/image` takes them; VHS nodes read from the
  same input dir).
- `prepare_workflow` recognises `VHS_LoadVideo`, `VHS_LoadVideoPath` and core `LoadVideo`.
- Source selection gains modes: `first frame`, `last frame`, `t = <seconds>`, `keyframe N`,
  `whole video`. `for_each_video_keyframe` (`scanner.rs:914`) is the existing seek primitive;
  extend it to arbitrary timestamps and last-frame.
- **Role binding for multiple image inputs** (closes G4): a stage declares which node id is
  `start`, `end`, `reference`. Default heuristic reads `_meta.title` (closing G7); explicit
  binding overrides.

**Acceptance:** a clip is extended by feeding its own last frame to an i2v stage, and a 720p clip
is upscaled to 4K, both end-to-end.

### FR3. Node introspection

- Call ComfyUI `/object_info` once per session, cached, invalidated on health-check reconnect.
- Use it to resolve, per `class_type`: input names, types, defaults, min/max, and **enum contents**
  (checkpoints, samplers, schedulers, LoRAs, upscale models installed on that box).
- Replaces all class-name substring sniffing in `detect_inputs` (closes G6).
- Feeds FR4 (typed widgets) and FR6 (template readiness).

### FR4. Typed parameters and fan-out

- The Enhance/line editor renders real controls: number steppers with the node's own min/max,
  dropdowns populated from the box's installed models, seeds with a policy of
  `fixed | random | increment`.
- **Fan-out**: `{"seed": 4}` produces 4 tasks in one run. Extendable to any enumerable parameter
  (`{"cfg": [4.0, 6.0, 8.0]}`).
- Parameter values are stored per task alongside the existing `text_overrides`, so a run is
  reproducible.

### FR5. Lines — runtime

- New tables `production_lines` and `line_stages` (ordered, each referencing a workflow with its
  input bindings and parameter overrides).
- `enhancement_tasks` gains `run_id`, `stage_idx`, `parent_task_id`.
- Worker change is small: on completion, look up the next stage and insert a task with
  `source_file_id = <this task's output_file_id>`. Both columns already exist.
- A failed stage fails the run, holding completed intermediate outputs for inspection; retry
  resumes from the failed stage, not the start.
- **Intermediate outputs are kept or discarded per stage, by the user's choice.** Default: keep
  while the run is live, then discard on completion unless the stage is marked `keep`. A stage
  feeding a hold point (FR5c) always keeps — choosing among its outputs is the entire point.

**Scope for v1: lines are linear.** Fan-out is allowed at any stage and propagates — each variant
runs the remainder of the line independently. Fan-*in* happens only at a hold point (FR5c), where a
person picks. True branching, where one stage feeds two divergent chains, is deferred.

### FR5a. Stage contracts

Before a workflow can be a stage in a line, Phos has to know what it accepts and what it produces.
On import, derive a **contract** from the workflow JSON plus `/object_info` (FR3):

```
accepts:  image | video | text | none  -- LoadImage / VHS_LoadVideo / text input / neither
produces: image | video | text         -- from the output node's class
roles:    start → node 12, end → node 14, reference → node 9
slots:    positive, negative           -- text inputs a describe stage or the user may fill
params:   seed, steps, cfg, frames     -- typed, with the node's own ranges
```

`text` is a first-class type because the prompt compiler is itself a stage (FR9). A text-producing
stage does not create a `files` row; its value binds to a downstream stage's slot.

Derived automatically, shown at import, and **user-correctable in place** — the heuristics will be
wrong on unusual graphs, and a wrong contract must be a two-click fix rather than a re-import.

The contract is what makes everything below possible: the stage picker can filter, the editor can
validate, and a mismatched line is rejected at design time instead of at dispatch.

### FR5b. Line authoring

Three paths, for three different moments. A user should almost never start from blank.

**1. Fork a template** — the default path, expected to cover most real use. Open a bundled line,
`Duplicate`, swap a stage, change a parameter, save.

**2. Compose in the line editor** — a vertical list, not a canvas. Phos does not draw graphs;
ComfyUI does. `Add stage` opens a picker that **only offers stages whose `accepts` matches the
previous stage's `produces`**, so an invalid line cannot be built in the first place. Between
stages sits a connector stating the handoff, clickable when there is a real choice:

```
  [1]  PHOTO → 5S CLIP              image → video
   │   whole video ▾
  [2]  INTERPOLATE 60FPS            video → video
   │   whole video ▾
  [3]  UPSCALE 4K                   video → video
```

The connector's options are FR2's source modes — `whole video`, `last frame`, `first frame`,
`t = <seconds>`. It resolves itself when unambiguous and only asks when the next stage declares
more than one role, reusing FR2's role binding rather than inventing a second mechanism.

Each stage row also carries its parameter disposition: `pinned` (fixed value), `exposed` (asked at
send time), `compiled` (filled by FR9), or `varied` (fan-out).

**3. Promote from history** — the highest-value path, and nearly free. Phos already records
`source_workflow_id` on every generated file, and FR5 adds `parent_task_id`. So it can detect a
repeated hand-run sequence and offer:

> *You ran Restore → Upscale → Interpolate on 12 shots. Save as a line?*

Zero design effort, and it captures what the user actually does rather than what they would have
thought to specify.

### FR5c. Hold points

A stage may be marked **hold for review**. The run parks after that stage, its outputs land in the
Takes lane, and it waits for a human verdict. This is the mechanism that makes fan-out economical:

```
  [1]  EXTEND CLIP +5s      ×4 seeds      ⏸ hold for review
   │   ↑ four candidate continuations land in Takes
  [2]  UPSCALE 4K                          ← only what you keep pays for this
```

Generate four continuations cheaply at 720p, look at them, and only then spend an hour upscaling.
Without hold points a line is fire-and-forget and every variant pays full cost; with them, curation
is a step *inside* the pipeline rather than a bin at the end of it.

**Three verdicts**, all available on any held run:

| Verdict | Effect |
|---|---|
| **Continue** | Proceed with the selected takes. **The selection may be more than one** — keep two of four and both run the remainder of the line independently. A hold is therefore also a fan-out point, and the estimate updates to show what continuing will cost. |
| **Regenerate** | Re-run the held stage with **fresh seeds and nothing else changed** — same prompt, same parameters, same source. The run stays alive and holds again on the new outputs; previous takes are kept or discarded by the stage's `keep` policy. Wanting different parameters is an edit and a new run, not a regenerate, which keeps the verdict a single unambiguous button. |
| **Cancel** | Abandon the run. Intermediate files are removed unless their stage is marked `keep`. |

Held runs show on the board as `HELD · 4 TAKES`, survive a restart, and are never silently
discarded or expired — a hold with no verdict stays held.

**Holds must not stall a batch.** At scale a hold point is the obvious way to deadlock the GPU:
3,329 shots through `×4 extend → hold → upscale` produces 13,316 clips waiting on a human before any
upscale runs. Two rules prevent that:

1. A held run **parks**; the batch keeps feeding new work past it. Held runs accumulate in Takes,
   they do not block the queue.
2. A batch carries a **cap on outstanding holds**. When more runs are held than the cap allows, the
   batch pauses feeding until verdicts bring the number down — so the farm never generates a
   mountain of candidates nobody has looked at.

Takes therefore supports bulk verdicts: decide by run, and apply a verdict to the remaining runs of
the same batch.

### FR5d. Lines belong to a library, and travel by export

A line lives in its library's `.phos.db`, like everything else Phos knows — consistent with the
no-global-database principle, so a line travels with the files it was built for. Bundled templates
are seeded into each library on first run.

A line **exports as one JSON file** bundling the line, each stage's workflow, the contracts and the
requirements manifest — the same format the bundled templates ship in, so there is one format rather
than two. That export is how a line moves to another library or another install. Import checks
requirements against `/object_info` and reports what is missing before anything runs.

### FR6. Bundled templates

Five, opinionated, shipped as JSON in the repo, seeded on first run, version-tracked so updates
do not clobber user edits:

| Template | Input → Output |
|---|---|
| Restore & Upscale (photo) | image → image, 4× |
| Photo → 5s clip | image → 720p video |
| Extend clip +5s | video → video, continued from last frame |
| Video → 4K | video → video, upscaled |
| Interpolate to 60fps | video → video, frame-interpolated |

Each carries a manifest of `required_nodes` and `required_models`, checked against `/object_info`
(FR3) and rendered with the existing status-colour vocabulary. A template that cannot run must say
exactly what to install — never fail at dispatch time.

**Templates stay updatable until you edit them.** A seeded workflow carries a `_phos` block in its
JSON — `template_key`, `template_version`, and a content hash of the workflow exactly as shipped:

```json
"_phos": { "template_key": "photo-to-clip", "template_version": 3, "hash": "sha256:…" }
```

On a Phos upgrade, each seeded workflow is rehashed. **Hash still matches → untouched → updated in
place.** **Hash differs → the user edited it → the `_phos` block is dropped, the workflow becomes an
ordinary imported one, and no future update will ever touch it.** The hash is the mechanism, so
nothing depends on an editor remembering to clear a marker; dropping the block on mismatch just
makes the state explicit and cheap to read afterwards.

The result: bundled templates improve over releases for the people who never customised them, and
are never clobbered for the people who did.

### FR7. Batch by query

- `POST /api/comfyui/runs` takes `{ line_id, selection, fanout, priority, skip_if_generated }`
  where `selection` is **either** an explicit id list **or** a query in the shape `/api/shots`
  already accepts (person, date range, review status, search text).
- A `batches` row holds the query plus a cursor; the worker materialises the next N tasks each
  tick. Fifty thousand task rows are never inserted at once — STOP stays instant, the board stays
  fast, and re-running tomorrow picks up newly imported matches for free.
- `skip_if_generated` filters against `files.source_workflow_id` / the existing `/generations`
  data. At batch scale this is a filter, not the warning dot `EnhanceDialog` shows today.
- Web gallery gains multi-select (`OrganizeDashboard.vue` has none; Android got it in `1ca2ca3`).
  **Query selection ships first** — it is both cheaper and the correct primitive at this scale.

### FR8. Scheduling and budget

- Configurable in-flight limit (default 1); read `/queue` depth before dispatch as backpressure.
**Order the pending queue by stage, not by run.** All tasks of one workflow drain before the next
workflow starts — every `describe` in the batch runs, then every `generate video`, then every
`upscale`. Two reasons, and the second is the larger one:

- **The model stays loaded.** On a 24 GB card, alternating a describe job with a 14B video job
  reloads ~20 GB per task. Draining by workflow means each model is loaded once per pass.
- **A pass completes as a unit.** Three thousand descriptions finishing together is a thing you can
  review in one sitting; three thousand runs each halfway through their own chain is not.

The tradeoff is deliberate: **runs advance in lockstep by stage rather than completing one at a
time**, so a batch trades per-run latency for throughput. That is the right trade for a farm and the
wrong one for a person waiting, which is why interactive priority still cuts the line — a single
click never queues behind three thousand descriptions.

It also composes with hold points: every describe finishes, you review the prompts in bulk, and only
then does any video generation start.

Remaining controls:

- Priority: `interactive` (a person clicked) always preempts `batch`.
- Configurable in-flight limit (default 1), with `/queue` depth read as backpressure.
- Disk floor: pause a batch when free space on the library volume drops below a threshold.
- Outstanding-hold cap: pause feeding when too many runs are awaiting a verdict (FR5c).
- An optional window (e.g. "only between 00:00 and 07:00") and a tasks/day cap, throttling a batch
  the user already started.

**Nothing in Phos initiates work on a timer.** There are no scheduled jobs, no cron, no standing
orders: a batch exists because a person started it. A window only paces work that is already
queued.

### FR9. Prompt compiler — a stage, not a service

**Qwen runs inside ComfyUI, as a workflow, like every other stage.** A describe stage takes
photo(s) plus a text instruction and returns text. No second service, no `PHOS_LLM_URL`, no extra
model host: the GPU box already exists, and the describe workflow is editable in ComfyUI exactly
like the generation ones.

Consequences, in dependency order:

1. **Text becomes a first-class stage type** (FR5a). `produces: text` joins image and video. A
   text output does not create a `files` row; it binds into a downstream stage's slot.
2. **The history reader must handle inline text outputs.** ComfyUI publishes them under a node's
   own output key (`text`, `string`, or whatever a custom node picks). FR1's generic output
   scanning already refuses to hard-code keys for files; it must do the same for inline values.
   Previously filed as "worth doing anyway" — now load-bearing.
3. **A line can describe, then generate:**

   ```
     [1]  DESCRIBE (Qwen-VL)        image → text
      │   text → positive
     [2]  PHOTO → 5S CLIP           image + text → video
   ```

4. **Phos supplies what Qwen cannot see.** The instruction sent into the describe workflow carries
   the person names from clustering, the EXIF date and place, the user's one-line intent, the style
   preset, and the stage's `do_not` constraints. Qwen supplies the looking; Phos supplies the
   knowing. Asking for structured output keeps it usable:

   ```json
   { "subject": "…", "setting": "…", "lighting": "…", "camera": "…",
     "motion_affordance": "hair and water could move; the subject is seated",
     "do_not": ["change face", "add people"] }
   ```

5. **Results cache per shot** in `shots.analysis_json`, so a second line over the same shot does not
   pay for the description again.

**Reviewing the prompt before the expensive stage.** For a single interactive shot, Phos runs the
describe stage first — seconds — and shows its output in the dialog for editing before the costly
stage is queued. At batch scale, marking the describe stage `hold for review` (FR5c) gives the same
control, or you let it run unattended. Either way the compiled prompt is **stored on the task**: a
prompt you cannot see or correct is worse than one you typed.

Florence-2 stays where it is — the library-search caption in `shots.description`. It is not the
prompt author.

### FR10. Curation, provenance, and synthetic marking

- **Takes lane** as described in UX.
- **Manifest** per generated file: line id, stage index, parent file id, seed and all parameters,
  compiled prompt, model checkpoint, ComfyUI prompt id. Stored in the DB and as a sidecar so it
  survives a database rebuild.
- **`synthetic` flag** on `files`, distinct from `is_original`. It drives three things:
  1. An AI-generated badge in web and Android.
  2. An XMP `DigitalSourceType = trainedAlgorithmicMedia` marking **written into the file at
     generation time, not at export**. Provenance that lives only in the database is lost the
     moment a row is deleted or a `.phos.db` is rebuilt — and a rescan then re-imports the file as
     an ordinary photograph, which is exactly the outcome this requirement exists to prevent.
     Marking the bytes makes the fact travel with the file, the way everything else in Phos does,
     and lets a rescan *recover* the flag rather than lose it.
  3. **Exclusion from face indexing and person clustering.** This is a correctness requirement,
     not a nicety: at farm scale, generated faces feeding ArcFace would drift every person
     centroid. Today's exclusion is incidental — the worker bypasses analysis — and must become
     enforced.

---

## Data model changes

```
production_lines(id, name, description, template_key, template_version, created_at)
line_stages(id, line_id, idx, workflow_id, input_bindings_json, param_overrides_json,
            fanout_json, source_mode, hold_for_review, keep_output)
runs(id, line_id, shot_id, batch_id, status, held_at_stage, created_at)
run_holds(id, run_id, stage_idx, verdict, kept_file_ids_json, note, decided_at)
batches(id, line_id, selection_json, cursor, priority, caps_json,
        max_outstanding_holds, status, created_at, stopped_at)
saved_selections(id, name, line_id, selection_json, created_at)
node_info_cache(comfyui_url, fetched_at, object_info_json)

enhancement_tasks  += run_id, stage_idx, parent_task_id, batch_id, priority,
                      params_json, compiled_prompt_json, text_output,
                      output_prefix, settle_until
files              += synthetic (bool), manifest_json, run_id, stage_idx, intermediate (bool)
shots              += analysis_json          -- cached describe-stage output
comfyui_workflows  += contract_json, requirements_json,
                      source ('imported' | 'bundled'), bundled_version
```

`enhancement_tasks.text_output` holds a text stage's result, which never becomes a file.
`files.intermediate` drives the per-stage keep/discard policy in FR5. `run_holds` is append-only, so
a run that was regenerated three times keeps the record of each verdict.

All of these live in the library's own `.phos.db` — lines are library-scoped (FR5d), so there is no
new global state anywhere.

New task statuses: `awaiting_output`, `cancelled`. New `files.synthetic` must be backfilled from
`source_workflow_id IS NOT NULL`.

---

## API changes

| Endpoint | Purpose |
|---|---|
| `POST /api/comfyui/tasks/{id}/cancel` | Missing today; frontend already calls it |
| `GET /api/comfyui/nodes` | Cached `/object_info`, typed widgets and installed model lists |
| `GET/POST /api/comfyui/lines`, `PUT/DELETE /api/comfyui/lines/{id}` | Line CRUD |
| `GET /api/comfyui/templates` | Bundled set with per-template readiness |
| `POST /api/comfyui/runs` | Run a line over a selection or query; returns a run or batch id |
| `GET /api/comfyui/runs`, `GET /api/comfyui/runs/{id}` | Board data, one row per run |
| `POST /api/comfyui/batches/{id}/stop` | Stop and purge from ComfyUI's queue |
| `POST /api/comfyui/lines/{id}/export`, `POST /api/comfyui/lines/import` | One JSON file carrying the line, its workflows, contracts and requirements |
| `GET /api/comfyui/lines/suggested` | Repeated hand-run sequences detected from history, offered as lines (FR5b) |
| `POST /api/comfyui/runs/{id}/verdict` | Decide a held run: `{action: continue \| regenerate \| cancel, keep_file_ids: [...]}` — `continue` accepts more than one take |
| `POST /api/comfyui/batches/{id}/verdict` | Apply one verdict to the remaining held runs of a batch |
| `GET/POST /api/comfyui/saved-selections` | A query plus its usual line, for one-click repeat batches |
| `POST /api/comfyui/estimate` | Pre-flight counts, GPU hours, disk, skip counts |
| `GET /api/files/{id}/manifest` | Provenance for a generated file |
| `POST /api/takes/{file_id}/verdict` | Keep / reject / rate / promote |

`android/openapi.json` is regenerated from `cargo run -- openapi` on every API change; the Retrofit
client is generated from it and never hand-written.

---

## Risks

### Product risks

- **The farm produces more than anyone will ever review.** Mitigation: the outstanding-hold cap and
  the daily cap are v1 requirements, not settings added later; the Takes lane is designed for speed
  of rejection; and nothing runs unless a person started it.
- **Synthetic memories contaminate a family archive.** Mitigation: FR10, treated as a correctness
  requirement with UI, metadata, and clustering consequences.
- **Bundled templates rot** as custom nodes change their APIs. Mitigation: readiness checks fail
  loudly and specifically at import, not silently at dispatch.

### Technical risks

- **ComfyUI has no stable contract.** Output keys, node class names, and history semantics vary by
  version and by installed extensions. Mitigation: generic output scanning (FR1), `/object_info`
  introspection instead of assumptions (FR3), and deterministic filenames as the recovery path.
- **VRAM thrash makes throughput far worse than the estimate.** Mitigation: stage-ordered
  scheduling (FR8) so each model loads once per pass, and estimates calibrated from measured task
  durations rather than guesses.
- **Disk.** 4K video output at farm rates fills terabytes. Mitigation: disk floor, intermediate
  cleanup, reject-deletes-bytes.
- **Migration weight.** Roughly a dozen new columns across four tables. Mitigation: one migration
  per milestone, each independently revertible.

### Project risks

- **Scope.** FR1–FR10 is a lot. Mitigation: the milestones below are ordered so each one is
  independently useful; stopping after M3 still leaves a materially better product.

---

## Milestones

### M1 — Reliability floor *(2–3 days, in flight)*
FR1. No new features. Success: a completed ComfyUI job is never reported as failed; failures carry
their real cause; cancel works.

### M2 — Introspection and video *(4–5 days)*
FR2 + FR3. Success: a 720p clip is extended and upscaled to 4K end-to-end; parameter widgets show
the actual models installed on the box.

### M3 — Lines, the line editor, and templates *(6–8 days)*
FR4 + FR5 + FR5a–d + FR6. Success: one click on a shot produces a chained i2v → interpolate →
upscale result; a user builds their own line in the editor without being able to construct an
invalid one; a `×4 extend → hold → upscale` line parks in Takes and resumes with the kept take;
five bundled templates report readiness honestly on a fresh install.

### M4 — Scale *(~1 week)*
FR7 + FR8. Success: 10,000 shots sent to a line from a query, draining stage by stage so each model
loads once per pass, under a daily cap and an optional overnight window, stoppable in one click.

### M5 — Curation and provenance *(~1 week)*
FR9 + FR10. Success: prompts are compiled from library knowledge and editable before dispatch;
takes are reviewed at a rate of several per second; every generated file is marked synthetic and
carries a manifest that reproduces it.

---

## Delivery — one PR per requirement

Requirements land as stacked pull requests, one per FR or FR part, each branching off the last
merged ancestor rather than off `master`. The order is a dependency graph, not a single line:

| Wave | PR | Depends on | Notes |
|---|---|---|---|
| — | **FR1** reliability floor | — | PR #107, plus the module split |
| A | **FR2** video in / out | FR1 | Independent of FR3; unblocks extend, 4K, interpolate |
| A | **FR3** `/object_info` introspection | FR1 | Gates FR4, FR5a, FR6 |
| A | **FR10a** synthetic flag + manifest | FR1 | **Deliberately early.** Excluding generated faces from clustering is a correctness fix that must land *before* anything generates at volume, not after |
| B | **FR4** typed params + fan-out | FR3 | |
| B | **FR5a** stage contracts | FR2, FR3 | Video types only mean something once FR2 exists |
| C | **FR5** lines runtime | FR5a | **The bottleneck** — every wave below waits on this one PR |
| D | **FR5b** line editor | FR5, FR5a | |
| D | **FR5c** hold points | FR5, FR4 | Fan-out is what makes a hold worth having |
| D | **FR5d** export / import | FR5, FR5a | |
| D | **FR6** bundled templates + `_phos` | FR5, FR3 | A template *is* a line |
| D | **FR9** describe stage | FR5a, FR5 | Needs the `text` type and a two-stage line |
| E | **FR7** batch by query | FR5 | |
| E | **FR8** stage-ordered queue | FR5, FR7 | Only matters at batch scale |
| E | **FR10b** Takes curation lane | FR5c, FR10a | |

Fourteen PRs across five waves. Within a wave the PRs are independent and can be built in parallel;
between waves they cannot. Each PR carries its own migration, tests, and — where it changes the API
surface — a regenerated `android/openapi.json`.

**Review between waves.** Fourteen PRs of compounding unreviewed work is the obvious way for one
early design mistake to end up baked into thirteen others. A wave is the right review unit.

---

## v1 success metrics

- **Zero** completed ComfyUI jobs reported as failed over a 1,000-task batch.
- A 10,000-shot batch runs unattended overnight and reports an accurate count in the morning.
- Time from "I want a clip of this photo" to a queued, correctly-prompted task: **under 5 seconds,
  zero typing**.
- Curation throughput: **≥ 200 takes reviewed in 10 minutes**.
- A bundled template on a fresh ComfyUI install either runs, or names exactly what is missing —
  never fails at dispatch.
- 100% of generated files carry the synthetic flag and a reproducing manifest.

---

## Open questions

1. **What is the right default cap on outstanding holds?** Too low and the GPU idles waiting for
   attention; too high and you wake to thirteen thousand unreviewed clips. Needs measuring against a
   real batch rather than deciding on paper.

### Decided

| Question | Decision |
|---|---|
| Where does Qwen run? | **Inside ComfyUI**, as a describe stage — no external service (FR9) |
| Keep intermediate outputs? | **User's choice, per stage**; hold points are how you choose among them (FR5, FR5c) |
| Can a line branch? | **No** for v1 — linear, with fan-out and hold points instead |
| Library or installation scope? | **Library.** Lines live in `.phos.db` and travel by export (FR5d) |
| Anything scheduled on a timer? | **No.** No cron, no standing orders — a batch exists because a person started it (FR8) |
| How is the queue ordered? | **By stage.** Every task of one workflow drains before the next, so the model loads once per pass (FR8) |
| How do templates update without clobbering edits? | **A `_phos` block with a content hash.** Match → update; mismatch → drop the block, never touch it again (FR6) |
| What does `regenerate` change? | **Seeds only.** Everything else stays; a parameter change is an edit and a new run (FR5c) |
| Takes on Android? | **Not in v1.** Desktop-only; a held run waits for a desk |
| Root cause of the reported flakiness? | Both candidates fixed in M1 — see PR #107 |

---

## Recommendation

Build M1 first and do not skip it. Everything downstream multiplies whatever error rate the
completion path has, and the current path fails a job because it looked for two specific JSON keys
and gave up after ten seconds.

Then M2, because `get_source_image` returning frame zero is the single line standing between the
current product and half of the stated goal — extend, 4K, interpolate are all impossible until a
video can enter a workflow as a video.

M3 is where it stops being a tool and starts being a product: one click, a chain of stages, and
templates that work without a ComfyUI tutorial.

M4 and M5 are what make it a *farm* — but they are also the two milestones that are easiest to
misjudge, because at ten thousand shots the constraints stop being about ComfyUI and start being
about disk, attention, and trust.
