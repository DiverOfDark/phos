# Multi-stage build for Phos
# syntax=docker/dockerfile:1

ARG PHOS_VERSION=dev

# Stage 1: Build Frontend
FROM node:20-slim AS frontend-builder
ARG PHOS_VERSION
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci
COPY frontend/ ./
RUN PHOS_VERSION=${PHOS_VERSION} npm run build

# Stage 2: Build Backend
FROM rust:1.94 AS backend-builder
RUN apt-get update && apt-get install --no-install-recommends -y \
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
COPY backend/build.rs ./
# Build dependencies with dummy source (cached separately from app code)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/backend/target \
    mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release --features "" || true && \
    rm -rf src

# Set PHOS_VERSION after dep build so version changes don't invalidate dep cache
ARG PHOS_VERSION
ENV PHOS_VERSION=${PHOS_VERSION}

COPY backend/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/backend/target \
    touch src/main.rs && cargo build --release && \
    cp target/release/phos-backend /usr/local/bin/phos-backend

# Stage 3: Final Image
FROM debian:trixie-slim
RUN apt-get update && apt-get install --no-install-recommends -y \
    libssl3 \
    libsqlite3-0 \
    ffmpeg \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 phos && useradd -u 1000 -g phos -m phos

WORKDIR /app

# Copy backend binary
COPY --from=backend-builder /usr/local/bin/phos-backend ./phos-backend

# Copy frontend build
COPY --from=frontend-builder /app/frontend/dist ./static

# Create directories writable by the app user
RUN mkdir models library && chown -R phos:phos /app

EXPOSE 3000
ENV PHOS_STATIC_DIR=/app/static
USER 1000
CMD ["./phos-backend"]
