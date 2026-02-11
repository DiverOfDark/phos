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
| `PHOS_STATIC_DIR` | `static` | Path to built frontend files |
| `PHOS_DUMMY_AI` | unset | Set to `1` to skip ONNX model loading (for testing without models) |
| `RUST_LOG` | unset | Tracing log level (e.g. `info`, `debug`) |

## Architecture

### Monorepo Layout
- **`backend/`** — Rust binary (Axum web server, SQLite, ONNX Runtime, FFmpeg)
- **`frontend/`** — Vue 3 SPA (Vite, Tailwind CSS 4, shadcn-vue/radix-vue)

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

### REST API Endpoints
- `GET /api/photos` — List all photos
- `GET /api/photos/:id` — Photo details with faces and files
- `GET /api/people` — List detected people/face clusters
- `GET /api/people/:id` — Photos of a specific person
- `POST /api/scan` — Trigger a library scan

## AI Models

Models are auto-downloaded from Hugging Face (`public-data/insightface`, path `models/buffalo_l/`) on first run and cached by `hf-hub`. No manual setup needed.

- `det_10g.onnx` — SCRFD face detection (input: 640x640)
- `w600k_r50.onnx` — ArcFace face recognition (output: 512-d embeddings)

Set `PHOS_DUMMY_AI=1` to skip model download entirely (useful for development/testing).
