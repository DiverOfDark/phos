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
COPY backend/migrations ./migrations
COPY backend/src ./src
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2c: Build deps + run tests
FROM chef AS backend-test
COPY --from=planner /app/backend/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

ARG PHOS_VERSION
ENV PHOS_VERSION=${PHOS_VERSION}
COPY backend/Cargo.toml backend/Cargo.lock backend/build.rs ./
COPY backend/migrations ./migrations
COPY backend/src ./src
RUN cargo test --release --lib

# Stage 2d: Build release binary (reuses compilation from test stage)
FROM backend-test AS backend-builder
RUN cargo build --release && \
    cp target/release/phos-backend /usr/local/bin/phos-backend

# Stage 2e: Build Android APK (bundled into the image, downloadable from the web UI)
FROM eclipse-temurin:17-jdk AS android-builder
RUN apt-get update && apt-get install --no-install-recommends -y wget unzip \
    && rm -rf /var/lib/apt/lists/*
ENV ANDROID_HOME=/opt/android-sdk
RUN mkdir -p ${ANDROID_HOME}/cmdline-tools && \
    wget -q https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip -O /tmp/cmdline-tools.zip && \
    unzip -q /tmp/cmdline-tools.zip -d ${ANDROID_HOME}/cmdline-tools && \
    mv ${ANDROID_HOME}/cmdline-tools/cmdline-tools ${ANDROID_HOME}/cmdline-tools/latest && \
    rm /tmp/cmdline-tools.zip && \
    yes | ${ANDROID_HOME}/cmdline-tools/latest/bin/sdkmanager --licenses > /dev/null && \
    ${ANDROID_HOME}/cmdline-tools/latest/bin/sdkmanager --install \
        "platform-tools" "platforms;android-36" "build-tools;36.0.0" > /dev/null
WORKDIR /app/android
COPY android/ ./
ARG PHOS_VERSION
# Signed release when the keystore_password secret is provided, unsigned otherwise.
# versionName/versionCode are derived from PHOS_VERSION when it is a semver tag.
RUN --mount=type=cache,target=/root/.gradle \
    --mount=type=secret,id=keystore_password \
    if [ -s /run/secrets/keystore_password ]; then \
        export KEYSTORE_PASSWORD="$(cat /run/secrets/keystore_password)"; \
    fi && \
    VER="${PHOS_VERSION#v}" && VERSION_ARGS="" && \
    case "$VER" in \
      [0-9]*.[0-9]*.[0-9]*) \
        MAJOR="${VER%%.*}"; REST="${VER#*.}"; MINOR="${REST%%.*}"; \
        PATCH="${REST#*.}"; PATCH="${PATCH%%[!0-9]*}"; \
        VERSION_ARGS="-PversionName=${VER} -PversionCode=$((MAJOR * 10000 + MINOR * 100 + PATCH))" ;; \
    esac && \
    chmod +x gradlew && \
    ./gradlew --no-daemon assembleRelease ${VERSION_ARGS} && \
    cp app/build/outputs/apk/release/app-release*.apk /phos.apk

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

# Copy Android APK (served at /phos.apk, linked from the settings UI)
COPY --from=android-builder /phos.apk ./static/phos.apk

# Create directories writable by the app user
RUN mkdir models library && chown -R phos:phos /app

EXPOSE 3000
ENV PHOS_STATIC_DIR=/app/static
USER 1000
CMD ["./phos-backend"]
