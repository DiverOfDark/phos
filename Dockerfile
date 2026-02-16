# Multi-stage build for Phos

# Stage 1: Build Frontend
FROM node:20-slim AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm install
COPY frontend/ ./
RUN npm run build

# Stage 2: Build Backend
FROM rust:latest AS backend-builder
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    clang \
    cmake \
    libsqlite3-dev \
    ffmpeg \
    libavcodec-dev \
    libavformat-dev \
    libavutil-dev \
    libswscale-dev \
    libswresample-dev \
    libavdevice-dev \
    libavfilter-dev \
    wget \
    unzip \
    nasm \
    yasm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app/backend
COPY backend/Cargo.toml backend/Cargo.lock ./
# Create dummy src/main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release --features "" || true && \
    rm -rf src

COPY backend/src ./src
RUN touch src/main.rs && cargo build --release

# Stage 3: Final Image
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y \
    libssl3 \
    libsqlite3-0 \
    ffmpeg \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy backend binary
COPY --from=backend-builder /app/backend/target/release/phos-backend ./phos-backend

# Copy frontend build
COPY --from=frontend-builder /app/frontend/dist ./static

# Models should be provided via volume or downloaded
# For now, create the directory
RUN mkdir models library

EXPOSE 3000
ENV PHOS_STATIC_DIR=/app/static
CMD ["./phos-backend"]
