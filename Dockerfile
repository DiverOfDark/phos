# Multi-stage build for Phos
# syntax=docker/dockerfile:1

ARG PHOS_VERSION=dev

# Stage 1: Build Frontend
FROM node:25-slim AS frontend-builder
ARG PHOS_VERSION
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci
COPY frontend/ ./
RUN PHOS_VERSION=${PHOS_VERSION} npm run build

# Stage 2a: Chef base (install cargo-chef + system deps)
FROM rust:1.94 AS chef
RUN apt-get update && apt-get install --no-install-recommends -y \
    pkg-config libssl-dev libclang-dev clang cmake libsqlite3-dev \
    ffmpeg libavcodec-dev libavformat-dev libavutil-dev libswscale-dev \
    libswresample-dev libavdevice-dev libavfilter-dev wget unzip nasm yasm \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app/backend

# Stage 2b: Generate dependency recipe
FROM chef AS planner
COPY backend/Cargo.toml backend/Cargo.lock backend/build.rs ./
COPY backend/src ./src
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2c: Build deps + run tests
FROM chef AS backend-test
COPY --from=planner /app/backend/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

ARG PHOS_VERSION
ENV PHOS_VERSION=${PHOS_VERSION}
COPY backend/Cargo.toml backend/Cargo.lock backend/build.rs ./
COPY backend/src ./src
RUN cargo test --release --lib

# Stage 2d: Build release binary (reuses compilation from test stage)
FROM backend-test AS backend-builder
RUN cargo build --release && \
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
