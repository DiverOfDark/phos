# Phos - AI Photo & Video Manager

Phos is a self-hosted AI-powered media manager that automatically indexes your photos and videos, detects faces, and groups them by person. It uses ONNX Runtime for face detection (SCRFD) and recognition (ArcFace), with per-directory SQLite databases so metadata travels with your files.

## Features

- **AI Face Detection & Recognition** — SCRFD detection + ArcFace embeddings, automatic clustering by person
- **Recursive Media Scanning** — SHA256 hashing, duplicate detection, automatic file organization
- **Video Support** — Keyframe extraction and face analysis on video files
- **ComfyUI Integration** — Optional image enhancement via ComfyUI workflows
- **Multi-User Mode** — OIDC/SSO authentication with per-user isolated libraries
- **Web UI** — Modern Vue 3 gallery with people browser, import dialog, and settings
- **CLI Tools** — `import` (local/remote) and `reorganize` subcommands

## Quick Start with Docker

The Docker image is published to GitHub Container Registry on every push to `master`.

```bash
docker pull ghcr.io/diverofdark/phos:latest
```

### Docker Compose

```yaml
services:
  phos:
    image: ghcr.io/diverofdark/phos:latest
    ports:
      - "33000:33000"
    volumes:
      - ./data/models:/app/models
      - ./data/library:/app/library
    environment:
      - RUST_LOG=info
      - PHOS_LIBRARY_PATH=/app/library
    restart: unless-stopped
```

Then run:

```bash
docker compose up -d
```

Open [http://localhost:33000](http://localhost:33000) in your browser. AI models are automatically downloaded from Hugging Face on first startup.

## Environment Variables

### Core

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_PORT` | `33000` | HTTP server port |
| `PHOS_LIBRARY_PATH` | `./library` | Root directory for media files and database |
| `PHOS_STATIC_DIR` | `static` | Path to built frontend files |
| `PHOS_DUMMY_AI` | *(unset)* | Set to `1` to skip ONNX model loading (for development/testing) |
| `RUST_LOG` | *(unset)* | Tracing log level (`info`, `debug`, `trace`, etc.) |

### OIDC / SSO Authentication

Setting `PHOS_OIDC_ISSUER` enables multi-user mode — each authenticated user gets their own isolated library subfolder with a separate SQLite database.

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_OIDC_ISSUER` | *(unset)* | OIDC provider issuer URL (e.g. `https://auth.example.com/realms/phos`) |
| `PHOS_OIDC_CLIENT_ID` | *(required)* | OIDC client ID |
| `PHOS_OIDC_CLIENT_SECRET` | *(required)* | OIDC client secret |
| `PHOS_OIDC_REDIRECT_URI` | `http://localhost:{port}/api/auth/callback` | OAuth2 redirect URI |
| `PHOS_OIDC_SCOPES` | `openid profile email` | Space-separated OIDC scopes |
| `PHOS_JWT_SECRET` | *(auto-generated)* | Secret for signing session JWTs. Auto-generated and persisted to `.phos_jwt_secret` if not set |
| `PHOS_JWT_TTL` | `3600` | Session JWT lifetime in seconds |

### ComfyUI Integration

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_COMFYUI_URL` | *(unset)* | ComfyUI server URL (e.g. `http://localhost:8188`). Enables background image enhancement |

### Docker Compose with SSO and ComfyUI

```yaml
services:
  phos:
    image: ghcr.io/diverofdark/phos:latest
    ports:
      - "33000:33000"
    volumes:
      - ./data/models:/app/models
      - ./data/library:/app/library
    environment:
      - RUST_LOG=info
      - PHOS_LIBRARY_PATH=/app/library
      - PHOS_OIDC_ISSUER=https://auth.example.com/realms/phos
      - PHOS_OIDC_CLIENT_ID=phos
      - PHOS_OIDC_CLIENT_SECRET=your-client-secret
      - PHOS_OIDC_REDIRECT_URI=https://phos.example.com/api/auth/callback
      - PHOS_COMFYUI_URL=http://host.docker.internal:8188
    extra_hosts:
      - "host.docker.internal:host-gateway"
    restart: unless-stopped
```

## Development

### Prerequisites

- Rust (latest stable)
- Node.js (v20+)
- System libraries: `clang`, `libclang-dev`, FFmpeg dev libs (`libavutil-dev`, `libavformat-dev`, `libavcodec-dev`, `libavdevice-dev`, `libavfilter-dev`, `libswscale-dev`, `libswresample-dev`)

### Backend

```bash
cd backend
cargo build
cargo run            # Starts server on port 33000
cargo test           # Run tests
```

### Frontend

```bash
cd frontend
npm install
npm run dev          # Vite dev server with HMR
npm run build        # Production build → dist/
npm test             # Run vitest
```

Set `PHOS_DUMMY_AI=1` to skip AI model download during development.

## Architecture

- **Backend** — Rust: Axum, Rusqlite, ORT (ONNX Runtime), FFmpeg
- **Frontend** — Vue 3, Vite, Tailwind CSS 4, shadcn-vue

AI models (`det_10g.onnx`, `w600k_r50.onnx`) are auto-downloaded from Hugging Face (`public-data/insightface`) on first run and cached locally.

## License

MIT
