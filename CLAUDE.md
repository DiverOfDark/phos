# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Phos is a self-hosted AI-powered photo/video manager. It scans directories, detects faces using ONNX models (SCRFD for detection, ArcFace for recognition), and groups media by person. Uses per-directory SQLite databases (`.phos.db`) so metadata travels with files.

## Build & Development Commands

### Backend (Rust, in `backend/`)
```bash
cd backend && cargo build          # Dev build
cd backend && cargo build --release # Release build
cd backend && cargo run             # Run dev server (port 33000)
cd backend && cargo test            # Run all tests
cd backend && cargo test scanner    # Run scanner tests only
# ComfyUI contract tests (skipped unless one of these is set):
docker build -t comfyui-test docker/comfyui-test
cd backend && PHOS_COMFYUI_TEST_IMAGE=comfyui-test:latest cargo test --test comfyui_contract_test -- --nocapture
cd backend && PHOS_COMFYUI_TEST_URL=http://localhost:8188 cargo test --test comfyui_contract_test   # against a running instance
```

### Frontend (Vue 3, in `frontend/`)
```bash
cd frontend && npm install    # Install dependencies
cd frontend && npm run dev    # Vite dev server with HMR
cd frontend && npm run build  # Production build → dist/
cd frontend && npm test       # Run vitest
```

### Docker
```bash
docker compose up --build    # Full stack (dummy AI mode by default)
```

### System Dependencies (needed for backend compilation)
- clang, libclang-dev
- FFmpeg dev libs: libavutil-dev, libavformat-dev, libavcodec-dev, libavdevice-dev, libavfilter-dev, libswscale-dev, libswresample-dev

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `PHOS_PORT` | `33000` | Backend HTTP port |
| `PHOS_S3_PORT` | unset | Also serve the read-only S3 API on a separate port at `/` |
| `PHOS_S3_PUBLIC_URL` | unset | External S3 endpoint URL shown in the settings UI |
| `PHOS_STATIC_DIR` | `static` | Path to built frontend files |
| `PHOS_DUMMY_AI` | unset | Set to `1` to skip ONNX model loading (for testing without models) |
| `RUST_LOG` | unset | Tracing log level (e.g. `info`, `debug`) |

## Architecture

### Monorepo Layout
- **`backend/`** — Rust binary (Axum web server, SQLite, ONNX Runtime, FFmpeg)
- **`frontend/`** — Vue 3 SPA (Vite, Tailwind CSS 4, shadcn-vue/radix-vue)
- **`android/`** — Kotlin/Compose app (Gradle, Hilt, Retrofit client generated from `android/openapi.json`)

### Backend Modules (`backend/src/`)
- **`main.rs`** — Entry point: initializes DB, AI pipeline, spawns background scan, serves static files + API
- **`api.rs`** — Axum REST routes under `/api/` (photos, people, scan trigger). State is `Arc<Mutex<Connection>>`
- **`db.rs`** — SQLite schema (tables: people, photos, files, faces, video_keyframes) and query functions
- **`ai.rs`** — ONNX face detection (SCRFD det_10g) and recognition (ArcFace w600k_r50) pipeline. Supports dummy mode via env var
- **`scanner.rs`** — Recursive directory walker: hashes files (SHA256), processes images/videos, runs face detection, stores results in SQLite
- **`comfyui/`** — ComfyUI integration, split so the code that *decides* is pure and testable without a server. `tests/comfyui_contract_test.rs` then pins the contract itself against a real CPU-only ComfyUI (`docker/comfyui-test/`, built and pushed by CI as `ghcr.io/<owner>/comfyui-test:<dockerfile-sha>`) with model-free core-node workflows. `history.rs` (what did ComfyUI say), `outputs.rs` (which files a run produced, or might have), `policy.rs` (how long to wait, and whether a failure is worth retrying), `params.rs` (a run's typed values, and what a swept one expands to), `workflow.rs` (graph analysis and rewriting), `contract/` (what a workflow accepts and produces), `prompt/` (the instruction a describe stage is sent, the answer read back, and the prompt compiled out of it), `line.rs` (whether a chain of them holds together, what travels along each join, what happens after a stage lands, what a verdict on a hold point may say, and whether a run is over), `editor.rs` (what the line editor may offer, and what a join has to be asked), `promote.rs` (which chains somebody has been running by hand often enough to be worth saving) and `portable/` (a line as a file, and what it needs installed) take `serde_json::Value` in and give an answer out; `client.rs` holds the HTTP calls and decides nothing; `runs.rs`, `holds/` (`mod.rs` reads a hold, `verdict.rs` writes what was decided), `takes/` (the curation lane's read model over *every* held run, plus what a verdict deletes and how far it reaches — `bulk.rs` is the pure rule for that), `api/line_io.rs` and `worker/` hold the DB writes and the background loop. Start at `comfyui/mod.rs` — its module doc has the task state machine, and `worker/advance.rs` has the run one

### Frontend Structure (`frontend/src/`)
- **`App.vue`** — App shell only: sidebar nav (topbar + lane tabs on mobile), import dialog, `<router-view>`
- **`components/ReviewDesk.vue`** — One screen, four lanes (`?lane=` → shots / duplicates / faces / takes); `/variations` redirects into it
- **`components/TakesQueue.vue`** — The Takes lane: a contact sheet over every run parked at a
  hold point, keyboard-first, fitted to the window so the footer that says what `⏎` is about to
  do never scrolls off. `lib/takes.js` holds the whole interaction model as a pure reducer —
  `keyAction` turns an event into an intent, `reduce` turns an intent into the next state plus
  effects — so the keyboard is tested by `node --test` rather than by driving a browser
- **`components/WorkflowsPage.vue`** — Three tabs: workflows, lines, and a queue that is a schedule
  board of **runs** — one row per run saying `STAGE 2/4 · UPSCALE · 00:03:12`, with the tasks
  underneath one click away. A four-stage run as four unrelated rows is what it replaced. A run
  parked at a hold point reads `HELD · 4 TAKES` and opens a review strip: its takes, tick the ones
  worth the stages below, and the three verdicts. That strip is deliberately just enough to give a
  verdict from without leaving the board; the keyboard-driven contact sheet over every held run at
  once is the Review Desk's Takes lane, and both read the same `POST`
- **`components/LineEditor.vue`** — A line, read as a route board and built as a vertical list.
  Read-only it draws like `WorkflowContract.vue`; under edit it is a list whose `Add stage` picker
  only offers what fits. `lib/lines.js` holds its bookkeeping and none of its rules
- **`components/SettingsPage.vue`** — Settings as a route (library path, WebDAV, S3, dedupe maintenance, APK)
- **`style.css`** — The AppBahn design system: raw tokens on `:root`, then a `@theme` block remapping Tailwind's palette, radii, shadows and fonts onto them, so utility classes render in the system. Semantic aliases (`bg-base`, `text-ink`, `border-line`, `text-signal`, `text-ready`…) are what components should use
- **`components/ui/`** — shadcn-vue primitives, no longer used by any screen (only the unreferenced `PhotoLightbox.vue`)
- **`lib/utils.js`** — `cn()` helper (clsx + tailwind-merge)
- **`lib/takes.js`** — The Takes lane's rules and none of its drawing: the key map as data, the
  reducer, what the next verdict will keep/reject/free, which parameters actually differ across a
  sheet's takes, and what to say about the batch a run came from. `batchOf` reads FR7's
  `GET /api/comfyui/batches` for a name and falls back to the batch id, so a missing or failing
  endpoint costs a label and never a rendered run; `batchNotice` prints FR7's own `paused_note`
  when a batch is paused on its outstanding-hold cap, because the person in this lane is the one
  whose verdicts lift it

### Design System — AppBahn (Bauhaus Engineering)
Both clients follow one system: dark by default, a single **signal amber** accent, status colour
(ready / degraded / error / pending / building) as the *only* other colour, hierarchy through font
weight rather than hue, 1px borders instead of shadows, 2px/4px radii and nothing pill-shaped, an
8px spacing grid, Geist / Inter / JetBrains Mono, and no gradients, glows or protection scrims.
Uppercase mono is the "railway schedule" register for labels, counts, ids and filenames.
- Web tokens live in `frontend/src/style.css`; fonts are bundled via `@fontsource-variable/*` so a
  LAN-only install still renders correctly
- Android tokens live in `android/.../ui/common/PhosTheme.kt` (`PhosColors` carries what Material 3
  has no slot for) with shared primitives in `PhosComponents.kt`; fonts are TTFs in `res/font/`
- Dynamic colour is deliberately off on Android: status colour carries meaning, so the wallpaper
  does not get a vote

### Key Design Decisions
- **A run is the unit, not a task.** `production_lines` + `line_stages` are a chain of workflows;
  `runs` is one chain applied to one shot; `enhancement_tasks.run_id` / `stage_idx` /
  `parent_task_id` make a task a step of one. A single-workflow enhance is a **one-stage run**, so
  the queue board has one kind of row and FR7's batch extends one endpoint rather than two.
  `runs.label` and `runs.stage_count` are snapshotted at creation, so a run still reads correctly
  after its line is renamed, edited or deleted
- **A stage advances by its output becoming the next stage's `source_file_id`.** The whole runtime
  is `comfyui/worker/advance.rs`: a completed task that is not at the last stage queues the one
  after it, and `parent_task_id` is the marker that says it already did — no "advanced" flag that
  could disagree with the thing it marks. Fan-out needs no special case: four completed takes each
  queue their own continuation, so four takes at stage 2 are four independent runners through 3
  and 4. **Order matters inside a tick** — continue, *then* settle. Settling first would call a run
  finished in the window before its next stage was written, and the sweep would delete the
  intermediate that stage was about to read
- **A failed stage fails the run, and a retry resumes from it.** The failed task queues no
  continuation, so the stages after it are never reached; nothing is cancelled and nothing is
  swept, so completed intermediates stay for inspection and sibling branches of a fan-out run on.
  Retrying re-queues only the failed tasks, each still holding the `source_file_id` it was given —
  re-running an hour of upscaling because stage 4 hiccupped is not something the code *can* do
- **Intermediates live exactly as long as they are useful.** Kept while the run is live (the next
  stage reads them, a failure wants them), swept when it *completes* — never when it fails.
  `line_stages.keep_output` overrides that per stage, the last stage's output is the product and is
  always kept, and a hold point's takes are kept because choosing among them is the entire point of
  the stage. `keeps_output` is a function of a `StageDisposition` (`is_final || keep_flag ||
  feeds_hold`) rather than a column read, so the two paths where the choosing does *not* stand — a
  run abandoned at its hold, and the generation a regenerate replaced — pass `feeds_hold: false`
  and get the keep flag's answer instead
- **A stage can park its run and ask.** `line_stages.hold_for_review` stops the line *after* that
  stage, puts its takes in front of a person, and goes on with the ones they keep — so
  `×4 extend → hold → upscale 4K` spends the hour of upscaling on the two clips somebody chose
  rather than on all four. Three verdicts and no fourth: **continue** with a selection (more than
  one is ordinary — a hold is a fan-*out* point as much as a filter), **regenerate** the held stage
  with fresh seeds and nothing else changed, **cancel** the run. Wanting different parameters is an
  edit and a new run, which is what keeps the verdict a button rather than a form
- **A hold is where fan-in happens, and `advance_after` is where the shape gave.** It used to be a
  total, local rule — a completed task not at the last stage continues, always. It now takes a
  `HoldGate` (*does this stage hold; was this take kept; was it reviewed at all*) and can answer
  `Advance::Hold`. Four takes converge on **one verdict**, which is the fan-in; the verdict fans
  back out to the kept subset, and that needs no new code because each kept take is an ordinary
  continuation. `run_holds` is append-only and carries **two** id lists: `kept_task_ids` and
  `reviewed_task_ids`. The second is not redundant — without it a passed-over take is
  indistinguishable from one nobody has seen, and the run parks again on it forever. It has exactly
  `parent_task_id`'s property: it exists precisely when the thing it marks happened
- **Reject arms; the verdict deletes.** The Takes lane's `X` marks a take and the bytes go when the
  verdict is sent, not on the keystroke. That is what lets the key cost one press and no dialog
  while staying reversible right up until it is not — and lets the footer print the megabytes the
  next `⏎` will free *before* it is pressed. The safeguard is a number always on screen, not a
  confirmation nobody reads. Rejecting is narrower than not-keeping: a take merely passed over is
  disposed of by its stage's `keep_output` policy, which is the line author's decision, while a
  rejected one goes regardless, which is the reviewer's. Both are recorded as *reviewed* on the same
  `run_holds` row
- **A verdict may cover a batch, but a rejection never travels.** `scope: "batch"` applies the same
  verdict to every other run of the same FR7 batch held at the same stage of the same line — you
  look at a handful of three thousand descriptions and let the rest through. `continue` resolves
  there to *all* of that run's own waiting takes, because task ids are per run and the selection
  made here does not exist there; `reject` is refused across a run nobody opened, because deleting
  bytes is something you do to pictures you have seen. Which runs a verdict covers is
  `comfyui/takes/bulk.rs`, pure and with no database near it. `runs.batch_id` is FR7's column and
  is read by a raw-SQL probe that treats the question failing as the answer "no batches", so a
  batch verdict is quietly a run verdict until FR7 lands
- **A held run parks; it never blocks.** Both halves of the advance pass filter on
  `status = running`, so a held run is read by nothing until a verdict releases it — 3,329 shots
  through a hold point park 3,329 runs and the queue keeps feeding the GPU from everything else.
  Held runs are never expired and never silently discarded: a hold with no verdict stays held.
  `runs.status = 'held'` and `runs.held_at_stage` survive a restart because they are columns rather
  than timers, and `RunState::live()` is what stops a line being edited under a held run
- **What crosses a join is one function, asked by four callers.** `line::carried_into` answers
  "what media type arrives at this stage", and the picker, the validator, the line reader that
  draws the connector and the dispatcher all ask *it* rather than agreeing with it. Two rules live
  inside it, both of which had been written more than once: a stage that produces **text** is
  transparent to the media flow (a describe stage makes no file, so the photograph it read is the
  photograph the stage after it reads), and a connector set to a **frame** of a clip —
  `first_frame` / `last_frame` / `at_time` / `keyframe` — hands on a still, which is what makes
  `photo → clip → restore` a line that can be built at all. `Accepts::admits` stays a pure
  media-type match: it is the primitive underneath, not the rule
- **A line is rejected when it is drawn.** Every join is `carried_into` and then `Accepts::admits`,
  on `POST`/`PUT`, again on every read, again when a run starts (with the shot's own type), and
  once more at dispatch against the file that actually turned up. A workflow can be re-imported or
  its contract corrected long after a line was built
- **A line travels as one file.** `comfyui/portable/`
  defines a `LineBundle`: the line and its ordered stages, **every stage's workflow graph** (a line
  exported as ids alone is a bundle of broken pointers), the derived contracts, and a requirements
  manifest of node classes and model files. Import checks those requirements against FR3's
  `NodeCatalog` and **reports what is missing before anything runs** — never at dispatch — but
  imports anyway, because the box holding the library is often not the box holding the GPU. An
  absent catalogue yields `unchecked`, not a wrong answer and not a refusal. Everything optional in the file
  (`contract`, `requirements`, per-stage overrides) can be omitted by a hand-written bundle. A
  `requirements` block in the file is documentation — the importer
  recomputes it from the graphs, because the graphs are what will actually be run. Names collide by
  suffixing, never overwriting; workflows deduplicate on the **canonical graph** (sorted keys, no
  whitespace), so a re-import reuses what is there
- **The stage picker asks the validator, it does not agree with it.** `comfyui/editor.rs` builds the
  line each candidate *would* make and hands it to `validate_chain`; offered means accepted, and the
  greyed row shows the validator's own sentence. Not a second rule to keep in step — which is why
  the two rules above reached the picker without a line of it moving. What is left in `editor.rs`
  is what there is to *decide* about a join rather than what it carries: which source mode, and
  which of a multi-input graph's slots. The browser holds no copy of any of it:
  `POST /api/comfyui/lines/stage-options` takes the draft and answers
- **A stage says one of three things about each setting.** *Pinned* (`parameters`), *varied*
  (`vary`, FR4's sweep) or *exposed* (`line_stages.exposed`) — asked for when the line is sent, and
  the only keys `POST /runs` will accept values for. Answers are snapshotted onto
  `runs.stage_values`, because the worker queues stage 4 long after the request that carried them.
  A key claiming two dispositions is refused. FR9's fourth, *compiled*, is a binding between two
  stages rather than a question for the sender, and belongs with the describe stage that fills it
- **A line a run is walking is locked, not versioned.** `GET /lines/{id}` reports `live_runs` and
  `editable` so the editor locks itself on load rather than discovering the `409` after ten minutes
  of typing — and offers Duplicate, which is the honest way to change a line something is currently
  reading
- **A workflow knows what it takes and what it gives.** `comfyui_workflows.contract_json` holds a
  `comfyui::StageContract`: `accepts` (image / video / text / **none**, because a text-to-image graph
  begins a line rather than continuing one), `produces` (image / video / text), which loader fills
  which slot, the prompt slots a person or a describe stage writes into, and the canonical settings
  (seed, steps, cfg, frames…) with the node's own ranges. It is what lets one workflow be chained
  after another. `text` is not a degenerate image: a describe stage creates no `files` row at all,
  its answer lands on `enhancement_tasks.text_output`, and it binds into the next stage's prompt
- **The prompt is compiled from the library, not retyped.** A *describe* stage is a workflow like
  any other — a vision-language model running inside ComfyUI, editable there like every other graph;
  there is no second service, no `PHOS_LLM_URL` and no LLM client in this tree. Phos writes its
  **instruction** (`comfyui/prompt/`): the person names clustering found, the EXIF date and place
  and the Florence-2 caption. The user's intent, the style preset and the `do_not` constraints stay
  *out* of it — the answer is cached per shot, so the description must be about the photograph and
  nothing else; the intent is folded in when the prompt is compiled, where changing it costs
  nothing. The model supplies the looking; Phos supplies the knowing. It answers with
  `{subject, setting, lighting, camera, motion_affordance, do_not}`, and that compiles to a positive
  prompt and a negative one — constraints never reach the positive prompt, because "do not add
  people" in a positive prompt adds people. Everything but the two DB reads is a pure function, so
  the wording, the parsing and the binding are tested with no ComfyUI and no GPU
- **A text stage is transparent to the media flowing down a line.** `describe → generate` type-checks
  because the describe stage made no file: the photograph it read is the photograph the stage after
  it reads, so a join is validated against the last stage that actually produced one and the
  continuation inherits its parent's `source_file_id`. The sentence goes in as **one `text_overrides`
  entry** — `StageContract::slot("positive").override_key()` is exactly the `"<node_id>.<field>"` key
  `prepare_workflow` already substitutes on, so binding a description needed no new plumbing at all
- **A description is paid for once per shot.** `shots.analysis_json` caches what a describe stage
  said (with which workflow, of which file, and when), and `dispatch` completes a describe task
  straight from it — no upload, no queued prompt, no GPU. The entry records the *file* it describes,
  and is a miss for any other: a mid-line describe stage reads the stage before it, and its answer
  must not stand in for the photograph's — nor a stale one for a file since promoted to original. A
  run that wants a fresh look sets the `phos:refresh` directive. The compiler's directives (`phos:intent`, `phos:style`, `phos:do_not`, `phos:slot`,
  `phos:refresh`) ride in `text_overrides` beside the `role:<node>` ones, so they are already stored
  on the stage, stored on the task, exported with a line and read by both the dispatch path and the
  advance pass. A stage inherits anything it did not say from the describe stage that fed it
- **Florence-2 stays where it is.** `shots.description` is the library-search caption and the prompt
  compiler *reads* it as one input among several. It is never the prompt, and nothing here writes it
- **A contract is derived, then corrected — never the other way round.** The derivation is
  heuristic and *will* be wrong on an unusual graph, so `contract_json` stores the corrections a
  person made alongside the derived answer, and `StageContract::derive_with` folds them back into
  the *next* derivation instead of patching its result. That is what lets "node 7 is the negative
  prompt" name a text box the heuristics never offered, and lets a contract worked out while
  ComfyUI was down be re-derived, properly typed, without losing what anyone said. The worker
  re-runs that pass every five minutes until the catalogue can be read
- **Generated media never enters the person model.** `files.synthetic` is the rule and the whole
  face pipeline reads it: `scanner.rs` skips detection, `cluster_faces` filters those faces out,
  the overlap sweep ignores those files, and a generated box cannot decide a shot's primary person.
  `purge_faces_on_synthetic_files` is the repair path for boxes that got in anyway (the watcher can
  index the bytes before the generator claims the row), and it recomputes the centroids they pulled
  on. A generated face averaged into an ArcFace centroid means re-clustering the whole library
- `files.manifest_json` holds a `comfyui::ProvenanceManifest` — a **versioned object with optional
  fields**, so later stages of the pipeline (line id, stage index, seed, compiled prompt) add to it
  without a migration. Unknown keys round-trip through `extra` rather than being dropped
- **The generated file's row is written before its bytes.** `files.path` is `UNIQUE`, so
  `comfyui/worker/store.rs` claims a name by inserting and treats the constraint failure as an
  answer, not an error. Checking for a free name and inserting later leaves a window that the write
  itself opens — once bytes are on disk a scan or the watcher can index the path first, and the
  task then fails with `UNIQUE constraint failed: files.path`. Reserving first also means the row
  already says `synthetic` by the time anything can find the file
- No global database — each root directory gets its own `.phos.db`
- AI models (ONNX) are auto-downloaded from Hugging Face (`public-data/insightface`) on first run and cached locally by `hf-hub`; startup fails hard if download fails (unless `PHOS_DUMMY_AI=1`)
- Backend serves the built frontend as static files via `fallback_service`
- API routes are mounted under `/api/`, everything else falls through to static file serving
- The Docker image also builds the Android app and ships the APK at `static/phos.apk`, so users can install it from the settings UI (`/phos.apk`). Release signing uses the `keystore_password` BuildKit secret; without it the APK is unsigned
- Android versioning feeds the in-app updater: `versionName` comes from `PHOS_VERSION` (a `v1.2.3` tag is stripped to `1.2.3`; otherwise `<branch>+<short-sha>`), and `versionCode` from the `PHOS_VERSION_CODE` build arg, which CI sets to `git rev-list --count HEAD` (monotonic on master; CI must check out with `fetch-depth: 0`). Both are passed to Gradle on **every** build — a fixed `versionCode` makes the updater compare 1 against 1 and answer "up to date" forever
- The `android-builder` stage writes `static/phos.apk.json` beside the APK (`version_name`, `version_code`, `sha256`, `size_bytes`) from the same values it gave Gradle, cross-checked with `aapt2 dump badging`. `GET /api/client/version` serves it — unauthenticated, like the APK download it describes, and always `200` (a build with no APK answers `available: false`). The app installs only a **strictly greater** `version_code`, after verifying the download's sha256 and that its signing certificate matches the running app's

### REST API Endpoints
- `GET /api/photos` — List all photos
- `GET /api/photos/:id` — Photo details with faces and files
- `GET /api/people` — List detected people/face clusters
- `GET /api/people/:id` — Photos of a specific person
- `POST /api/scan` — Trigger a library scan
- `PUT /api/import/upload` — Store an uploaded file and queue it for analysis (`202`; the analysis runs on the per-library ingest worker, not on the request)
- `GET /api/import/status` — Ingest queue depth for the caller's library, polled by the import UI
- `POST /api/faces/dedupe?dry_run=` — Collapse overlapping boxes drawn on one face (never merges two boxes assigned to different people; skips reviewed shots). Also runs at startup and after each upload batch is analyzed
- `PUT /api/comfyui/workflows/{id}/contract` — Replace the corrections applied to a workflow's derived stage contract, and get back the contract that results. Sending `{}` discards every correction and takes the derivation as it stands
- `GET /api/files/{id}/manifest` — Whether a file was generated, and the provenance manifest recording how. Answers for any file; a photograph comes back `synthetic: false` with no manifest
- `POST /api/comfyui/runs` — Start a line against a shot: `{ line_id, shot_id }`, plus optional `stage_values` answering what each stage left open. Queues stage 1; the worker queues each stage after it as the one before lands. FR7 replaces `shot_id` with a query and adds a cursor, which is why the handler already resolves a *set* of shots and answers with a list of runs
- `GET /api/comfyui/runs` — The queue board: one row per run, with the stage it is on, of how many, what that stage is running, and its clock. `GET /api/comfyui/runs/{id}` is the drill-down to the tasks underneath
- `POST /api/comfyui/runs/{id}/retry` — Resume from the stage that failed. What already succeeded is not re-run
- `GET /api/comfyui/runs/{id}/hold` — The takes a held run is waiting on, and what continuing from one of them costs in tasks. `null` for a run that is not holding
- `POST /api/comfyui/runs/{id}/hold` — The verdict: `continue` with the takes named (each walks the rest of the line for itself), `regenerate` the held stage with fresh seeds, or `cancel` the run. `reject` names takes whose files leave the library outright; `scope: "batch"` applies the verdict to the rest of the batch, carrying no rejection with it. `POST /runs/{id}/cancel` on a held run goes through this same path, so an abandoned hold is recorded like every other verdict
- `GET /api/comfyui/takes` — Every run waiting on a verdict, oldest first: one page of held runs with their takes, the file each take was made from, the shot's current main file, and what continuing costs. The Takes lane draws a whole screen from one request
- `PUT /api/files/{id}/rating` — One to five, or `null` to clear it. The Takes lane's `1`-`5` keys
- `GET|POST /api/comfyui/lines`, `GET|PUT|DELETE /api/comfyui/lines/{id}` — Line CRUD. A chain whose stages do not fit together is refused with a message naming the stage; editing or deleting a line is refused with `409` while a run of it is in flight
- `GET /api/comfyui/lines/{id}/export` — The line as one portable JSON bundle: stages, **the workflow graph behind each one**, the derived contracts, and a manifest of the node classes and model files it needs
- `POST /api/comfyui/lines/import?dry_run=&name=` — Read a bundle back. `dry_run` writes nothing and answers with the requirements report alone
- `POST /api/comfyui/describe` — Describe one shot and compile a prompt from it, together with the person names, EXIF and caption the library already holds. Answers instantly from `shots.analysis_json` unless `refresh` is set; otherwise queues a one-stage describe run. `GET /api/comfyui/describe/{shot_id}` polls it, and re-compiles for a different `intent`/`style`/`do_not` without describing the photograph again
- `POST /api/comfyui/lines/stage-options` — Which workflows may go in one slot of a line being edited. Send the draft's workflow ids, the position and `insert`/`replace`; every workflow comes back offered or refused, with the validator's own reason
- `POST /api/comfyui/lines/validate` — The same check `POST` and `PUT` make, with nothing written, plus each join's handoff. What the editor asks after a reorder
- `POST /api/comfyui/lines/{id}/duplicate` — Fork a line, with everything its stages carry, under a numbered name
- `GET /api/comfyui/lines/suggestions` — Sequences somebody has been running one workflow at a time, on enough different shots to be a habit, offered as lines ready to `POST`
- `GET /api/client/version` — Bundled Android APK metadata for the in-app updater (no auth)

## AI Models

Models are auto-downloaded from Hugging Face (`public-data/insightface`, path `models/buffalo_l/`) on first run and cached by `hf-hub`. No manual setup needed.

- `det_10g.onnx` — SCRFD face detection (input: 640x640)
- `w600k_r50.onnx` — ArcFace face recognition (output: 512-d embeddings)

Set `PHOS_DUMMY_AI=1` to skip model download entirely (useful for development/testing).
