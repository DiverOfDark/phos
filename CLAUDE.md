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

### Frontend Structure (`frontend/src/`)
- **`App.vue`** — Main shell: header, gallery, stats cards, settings sheet, import dialog
- **`components/ui/`** — shadcn-vue primitives (button, card, dialog, input, sheet, tabs, etc.)
- **`lib/utils.js`** — `cn()` helper (clsx + tailwind-merge)

### Key Design Decisions
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
- `GET /api/client/version` — Bundled Android APK metadata for the in-app updater (no auth)

## AI Models

Models are auto-downloaded from Hugging Face (`public-data/insightface`, path `models/buffalo_l/`) on first run and cached by `hf-hub`. No manual setup needed.

- `det_10g.onnx` — SCRFD face detection (input: 640x640)
- `w600k_r50.onnx` — ArcFace face recognition (output: 512-d embeddings)

Set `PHOS_DUMMY_AI=1` to skip model download entirely (useful for development/testing).
