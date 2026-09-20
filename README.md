# Phos - AI Photo & Video Manager

Phos is a self-hosted AI-powered media manager that automatically indexes your photos and videos, detects faces, and groups them by person. It uses ONNX Runtime for face detection (SCRFD) and recognition (ArcFace), with per-directory SQLite databases so metadata travels with your files.

## Features

- **AI Face Detection & Recognition** — SCRFD detection + ArcFace embeddings, automatic clustering by person
- **Recursive Media Scanning** — SHA256 hashing, duplicate detection, automatic file organization
- **Video Support** — Keyframe extraction and face analysis on video files
- **Multi-User Mode** — OIDC/SSO authentication with per-user isolated libraries
- **Web UI** — Modern Vue 3 gallery with people browser, import dialog, and settings
- **WebDAV Server** — Read-only network drive access to your library; mount from any file manager, Nextcloud, or rclone
- **MCP Server** — Point Claude (or any MCP client) at your library: it can search, read shots and people, and look at the photos themselves
- **CLI Tools** — `import` (local/remote), `reorganize` and `mcp` subcommands

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
| `PHOS_JWT_TTL` | `1209600` (14 days) | Session JWT lifetime in seconds |

### WebDAV

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_WEBDAV_PORT` | *(unset)* | Serve WebDAV on a separate port at `/` (e.g. `4918`). Useful for clients that have trouble with path-prefixed WebDAV |

### S3

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_S3_PORT` | *(unset)* | Also serve the S3 API on a separate port at `/` (e.g. `9000`). There `ListBuckets` works too, which the main port cannot offer |
| `PHOS_S3_PUBLIC_URL` | *(unset)* | External S3 endpoint URL shown in the settings UI (for reverse-proxy setups) |

### MCP

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOS_MCP_WRITE` | *(unset)* | Set to `1` to also expose the MCP tools that change the library (rename, merge, assign, confirm, scan). Read-only otherwise |
| `PHOS_PUBLIC_URL` | *(unset)* | External base URL, used to show the right `/mcp` endpoint in the settings UI (for reverse-proxy setups) |

### Docker Compose with SSO and WebDAV

```yaml
services:
  phos:
    image: ghcr.io/diverofdark/phos:latest
    ports:
      - "33000:33000"
      - "4918:4918"          # Optional dedicated WebDAV port
    volumes:
      - ./data/models:/app/models
      - ./data/library:/app/library
    environment:
      - RUST_LOG=info
      - PHOS_LIBRARY_PATH=/app/library
      - PHOS_WEBDAV_PORT=4918
      - PHOS_OIDC_ISSUER=https://auth.example.com/realms/phos
      - PHOS_OIDC_CLIENT_ID=phos
      - PHOS_OIDC_CLIENT_SECRET=your-client-secret
      - PHOS_OIDC_REDIRECT_URI=https://phos.example.com/api/auth/callback
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

## WebDAV Access

Phos includes a built-in read-only WebDAV server so you can mount your library as a network drive from any file manager (Finder, Explorer, Nautilus), Nextcloud, rclone, Cyberduck, etc.

1. Open **Settings** in the web UI and scroll to **WebDAV Access**
2. Set a username and password, then click **Enable**
3. Mount the displayed URL in your file manager or WebDAV client

The WebDAV endpoint is always available at `/webdav/` on the main port. Internal metadata files (`.phos.db`, thumbnails, etc.) are automatically hidden, and all write operations are rejected.

WebDAV (and S3) present a flattened view of the library: instead of the on-disk `{person}/{series}/{photo}.jpg`, each person folder is one flat list named `{person}/{series}_{photo}.jpg` — no per-series subfolders.

For clients that require WebDAV at the root path (e.g. older Windows Explorer), set `PHOS_WEBDAV_PORT` to serve WebDAV on a dedicated port at `/`.

### Example: mount with rclone

```bash
rclone mount :webdav: ~/phos-library \
  --webdav-url http://localhost:33000/webdav/ \
  --webdav-user myuser \
  --webdav-pass mypass \
  --read-only
```

## S3 Access

Phos also exposes the library through a read-only S3-compatible API, usable with rclone, the AWS CLI, Cyberduck, backup tools, etc.

1. Open **Settings** in the web UI and scroll to **S3 Access**
2. Click **Generate credentials** — the access key, secret key, endpoint, and bucket are displayed
3. Configure your S3 client with those values, region `us-east-1`, and **path-style addressing**

The bucket is always named `phos` and is served on the main port (the endpoint is the plain Phos URL, e.g. `http://localhost:33000`). Requests are authenticated with AWS Signature V4. Internal metadata files are hidden and all write operations are rejected.

Like WebDAV, S3 serves the flattened library view: object keys are `{person}/{series}_{photo}.jpg` rather than the on-disk `{person}/{series}/{photo}.jpg`.

Notes:

- The secret key is stored unhashed on the server — SigV4 verification requires the real secret. Rotate it anytime by clicking **Generate credentials** again.
- `ListBuckets` (`GET /`) is not available on the main port; clients must reference the `phos` bucket explicitly. Set `PHOS_S3_PORT` to serve the S3 API on a dedicated port where bucket listing also works.
- Reverse proxies must forward `/phos/...` unmodified — any path rewrite breaks request signatures.
- ETags are synthetic (mtime + size), so checksum-based verification won't work; size/modtime-based sync does.

### Example: sync with rclone

```bash
rclone sync ":s3,provider=Other,endpoint=http://localhost:33000,access_key_id=phos,secret_access_key=SECRET:phos" ~/phos-backup
```

### Example: AWS CLI

```bash
aws configure set default.s3.addressing_style path
AWS_ACCESS_KEY_ID=phos AWS_SECRET_ACCESS_KEY=SECRET \
  aws s3 ls s3://phos/ --endpoint-url http://localhost:33000 --region us-east-1
```

## MCP (Model Context Protocol)

Phos speaks MCP, so an AI assistant can work with the library directly — find the shots you mean, read what is in them, and actually look at the photos.

### Hosted server

1. Open **Settings** in the web UI and scroll to **MCP Access**
2. Click **Generate token** — the token is shown **once**, because only its SHA-256 is stored
3. Copy the `claude mcp add …` line the card gives you, or wire the endpoint into any MCP client by hand:

```bash
claude mcp add --transport http phos https://phos.example.com/mcp \
  --header "Authorization: Bearer phos_mcp_..."
```

The token travels in the `Authorization` header, never in the URL — a secret in a path is written to every access log and proxy log the request passes through, and cannot be rotated without reconfiguring every client. Rotate anytime from the same card; the old token stops working immediately.

In multi-user (OIDC) mode each user generates their own token, and it carries their identity, so a token only ever opens the library it was minted for.

### Local library

No server, no token — point the client at the binary:

```bash
claude mcp add phos -- phos mcp /path/to/library
```

Add `--allow-write` to enable the tools that change things. A read-only session never loads the ONNX models, so it starts immediately.

### What it exposes

Six read tools — `search_shots`, `get_shot`, `list_people`, `person_timeline`, `get_image`, `library_stats` — and, when writes are enabled, five more: `rename_person`, `merge_people`, `assign_face`, `confirm_shots`, `trigger_scan`. There is deliberately no delete tool.

Shots, people and files are also MCP **resources** (`phos://shot/{id}`, `phos://person/{id}`, `phos://file/{id}`), and searches answer with links to them rather than inlining every record. `resources/list` returns only the 100 most recent shots: a real library is far too large to enumerate into a context window.

Free-text search matches the AI-written caption and the file path, not the image contents.

## Architecture

- **Backend** — Rust: Axum, Rusqlite, ORT (ONNX Runtime), FFmpeg
- **Frontend** — Vue 3, Vite, Tailwind CSS 4, shadcn-vue

AI models (`det_10g.onnx`, `w600k_r50.onnx`) are auto-downloaded from Hugging Face (`public-data/insightface`) on first run and cached locally.

## License

MIT
