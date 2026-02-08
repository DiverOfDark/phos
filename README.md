# Phos - AI Photo & Video Manager

Phos is a self-hosted AI-powered media manager that automatically indexes your photos and videos, detects faces, and groups them by person using ONNX Runtime.

## Features

- **Recursive Media Scanning**: Automatically finds and hashes media files to prevent duplicates.
- **AI Face Detection**: Uses SCRFD for fast and accurate face detection.
- **AI Face Recognition**: Uses ArcFace to generate embeddings for clustering faces.
- **Video Support**: Keyframe extraction and analysis (Coming Soon).
- **Web UI**: Modern Vue 3 interface for browsing your gallery and people.

## Getting Started

### Prerequisites

- Rust (latest stable)
- Node.js (v18+)
- ONNX Models (det_10g.onnx and w600k_r50.onnx)

### Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/DiverOfDark/phos.git
   cd phos
   ```

2. Setup Backend:
   ```bash
   cd backend
   # Download models into backend/models/
   cargo run
   ```

3. Setup Frontend:
   ```bash
   cd frontend
   npm install
   npm run dev
   ```

## Architecture

- **Backend**: Axum (Web Framework), Rusqlite (Database), ORT (ONNX Runtime), FFmpeg (Video processing).
- **Frontend**: Vue 3, Vite, Tailwind CSS, shadcn-vue.

## License

MIT
