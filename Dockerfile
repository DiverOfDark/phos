# Multi-stage build for Phos
# syntax=docker/dockerfile:1
#
# Build:
#   DOCKER_BUILDKIT=1 docker build \
#     --build-arg PHOS_VERSION=v1.2.3 \
#     --build-arg PHOS_VERSION_CODE=$(git rev-list --count HEAD) \
#     --secret id=keystore_password,env=KEYSTORE_PASSWORD \
#     -t phos .
#
# PHOS_VERSION_CODE is the Android versionCode and MUST come from outside: no
# stage in here has the git history to count commits itself. CI passes
# `git rev-list --count HEAD`, which is monotonic on master rather than only on
# tags — the in-app updater compares that number against what the running APK
# reports, so a value that does not increase means "up to date" forever.

ARG PHOS_VERSION=dev
ARG PHOS_VERSION_CODE=1

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
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/build.rs ./
# Keep codegen conservative so the runtime works on older x86-64 CPUs too.
ENV RUSTFLAGS="-C target-cpu=x86-64-v2"

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
ARG PHOS_VERSION_CODE
# Signed release when the keystore_password secret is provided, unsigned otherwise.
#
# versionName AND versionCode are passed to Gradle on EVERY build, not only for
# semver tags. The old "derive them from PHOS_VERSION when it looks like semver"
# rule fell through on a master push (PHOS_VERSION is "master"), so every master
# image shipped versionCode 1 / versionName 1.0.0 and the in-app updater would
# compare 1 against 1 forever.
#
# versionCode is the commit count handed in by CI rather than anything derived
# from the tag: mixing the two schemes would let a tag (v1.0.0 -> 10000) outrank
# every subsequent master build, which is the same "never updates" failure with
# extra steps.
#
# The sidecar phos.apk.json is written from the SAME two values given to Gradle,
# and then checked against what aapt2 reads back out of the APK, so what the
# server advertises at /api/client/version cannot disagree with what the client
# will actually install. Its version_name is JSON-escaped because a git ref may
# legally contain a quote or a backslash, and unparseable metadata would take the
# update endpoint down with it.
RUN --mount=type=cache,target=/root/.gradle \
    --mount=type=secret,id=keystore_password \
    set -eu; \
    if [ -s /run/secrets/keystore_password ]; then \
        KEYSTORE_PASSWORD="$(cat /run/secrets/keystore_password)"; \
        export KEYSTORE_PASSWORD; \
    fi; \
    VERSION_NAME="${PHOS_VERSION:-dev}"; \
    case "$VERSION_NAME" in \
      v[0-9]*.[0-9]*.[0-9]*) VERSION_NAME="${VERSION_NAME#v}" ;; \
    esac; \
    VERSION_CODE="${PHOS_VERSION_CODE:-1}"; \
    case "$VERSION_CODE" in \
      ''|0|*[!0-9]*) \
        echo "PHOS_VERSION_CODE must be a positive integer, got '${VERSION_CODE}'" >&2; \
        exit 1 ;; \
    esac; \
    chmod +x gradlew; \
    ./gradlew --no-daemon assembleRelease \
        "-PversionName=${VERSION_NAME}" "-PversionCode=${VERSION_CODE}"; \
    cp app/build/outputs/apk/release/app-release*.apk /phos.apk; \
    BADGING="$(${ANDROID_HOME}/build-tools/36.0.0/aapt2 dump badging /phos.apk | head -1)"; \
    case "$BADGING" in \
      *"versionCode='${VERSION_CODE}'"*"versionName='${VERSION_NAME}'"*) ;; \
      *) echo "APK does not carry the requested version: ${BADGING}" >&2; exit 1 ;; \
    esac; \
    SHA256="$(sha256sum /phos.apk | cut -d' ' -f1)"; \
    SIZE_BYTES="$(stat -c %s /phos.apk)"; \
    NAME_JSON="$(printf '%s' "$VERSION_NAME" | sed 's/\\/\\\\/g; s/"/\\"/g')"; \
    printf '{"version_name":"%s","version_code":%s,"sha256":"%s","size_bytes":%s}\n' \
        "$NAME_JSON" "$VERSION_CODE" "$SHA256" "$SIZE_BYTES" > /phos.apk.json; \
    cat /phos.apk.json

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

# Copy Android APK (served at /phos.apk, linked from the settings UI).
#
# The sidecar next to it is what GET /api/client/version reads: the app compares
# its own BuildConfig.VERSION_CODE against version_code, and verifies the
# download against sha256 before installing it. It has to travel with the APK —
# metadata describing a *different* build is worse than none, because the client
# would reject every download as corrupt.
COPY --from=android-builder /phos.apk ./static/phos.apk
COPY --from=android-builder /phos.apk.json ./static/phos.apk.json

# Create directories writable by the app user
RUN mkdir models library && chown -R phos:phos /app

EXPOSE 3000
ENV PHOS_STATIC_DIR=/app/static
USER 1000
CMD ["./phos-backend"]
