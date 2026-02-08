# Phos Architecture

Phos is a localized, AI-powered photo and video management system designed to sit atop a directory structure synchronized via services like Nextcloud.

## Core Principles
- **No Global DB:** Uses SQLite per root directory (`.phos.db`) to ensure metadata travels with the files.
- **Local First:** Designed for low-latency local network usage.
- **AI-Driven Organization:** Automatically groups "processed" versions (crops, upscales, edits) with their originals and clusters media by detected faces.

## System Components

### 1. Backend (Rust)
Built with **Axum**, chosen for its modern async stack and performance.
- **File Scanner:** Watches the directory for changes (using `notify`) and performs initial hashing.
- **AI Pipeline (ONNX Runtime):**
    - **Face Detection:** Using SCRFD or RetinaFace models for high-accuracy face localization.
    - **Face Recognition:** Using ArcFace or similar to generate 512-d embeddings for clustering.
    - **Visual Fingerprinting:** Using a lightweight ResNet/MobileNet to generate embeddings for detecting "processed" variations of the same image.
- **Video Processor:** Extracts keyframes from MP4/MKV files using `ffmpeg` bindings for face analysis.
- **Metadata Manager:** Interfaces with SQLite to store hashes, embeddings, and relationships.
- **REST API:** Serves the frontend and potential future mobile clients.

### 2. Frontend (Vue 3 + Vite)
- **Tech Stack:** Vue 3, Pinia (state), Tailwind CSS.
- **PWA Ready:** Configured for "Install to Home Screen" on Android/iOS.
- **Features:** 
    - Infinite scroll gallery.
    - Face clustering view (People).
    - Grouped view (Original + Edits).
    - Drag-and-Drop importer.

### 3. Storage Strategy
The system enforces a specific directory structure:
```
Root/
├── .phos.db (SQLite)
├── Person Name/
│   └── Photo_ID/
│       ├── original.jpg
│       ├── upscale_v1.png
│       └── edit_v2.jpg
```

## Data Flow: The "Import" Lifecycle
1. **Detection:** New file detected in the "Inbox" or uploaded via UI.
2. **Fingerprinting:** Generate a visual embedding.
3. **Similarity Match:** Search the DB for existing embeddings with high cosine similarity (>0.85). If found, link as a "processed" version.
4. **Face Analysis:** If no similarity match, run face detection/recognition.
5. **Clustering:** 
    - If faces match an existing person, move to that person's directory.
    - If faces are new, move to an `Unknown_XXX` directory.
6. **Persistence:** Update `.phos.db` with the new file path and relationships.
