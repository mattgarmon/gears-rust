# Multi-stage build for the CF/Gears Flight Control image.
#
# Flight Control is the minimal platform control plane: gear-orchestrator,
# grpc-hub, api-gateway, types-registry, and the embedded authn-resolver. See
# docs/arch/toolkit-oop/DESIGN.md "Flight Control Composition". Application gears
# and the AuthZ plane run as their own OoP images.
#
# Build (static-authn default):
#   docker build -f deploy/docker/flight-control.Dockerfile \
#     -t ghcr.io/constructorfabric/flight-control:dev .
#
# Build (production: OIDC authn, k8s auth):
#   docker build -f deploy/docker/flight-control.Dockerfile \
#     --build-arg CARGO_NO_DEFAULT_FEATURES=1 \
#     --build-arg CARGO_FEATURES="oidc-authn k8s" \
#     -t ghcr.io/constructorfabric/flight-control:prod .

# ---------------------------------------------------------------------------
# Stage 1: Builder
# ---------------------------------------------------------------------------
FROM rust:1.95.0-bookworm@sha256:6bb82db0878825e157664188b319c875de4f1fff5d70f5917b3a3f1974b472e4 AS builder

# BUILD_PROFILE: "release" (default, optimized) or "dev" (fast compile).
ARG BUILD_PROFILE=release
# Extra cargo features to enable, space-separated (e.g. "prod-plugins k8s otel").
ARG CARGO_FEATURES=""
# Set to a non-empty value (e.g. "1") to pass --no-default-features (drops the
# default dev-plugins preset - required when building the prod-plugins image).
ARG CARGO_NO_DEFAULT_FEATURES=""

# protobuf-compiler is required by prost-build (gRPC / directory protos).
RUN apt-get update && \
    apt-get install -y --no-install-recommends cmake protobuf-compiler libprotobuf-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the full workspace context (.dockerignore trims target/, .git/, logs/).
COPY . .

# Build the flight-control binary. BuildKit cache mounts persist the cargo
# registry + target dir across builds; the binary is copied out to /tmp because
# the target dir is a cache mount and does not survive the layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    set -eux; \
    RELEASE_FLAG=""; OUTPUT_DIR="debug"; \
    if [ "$BUILD_PROFILE" = "release" ]; then RELEASE_FLAG="--release"; OUTPUT_DIR="release"; fi; \
    NO_DEFAULT_FLAG=""; \
    if [ -n "$CARGO_NO_DEFAULT_FEATURES" ]; then NO_DEFAULT_FLAG="--no-default-features"; fi; \
    FEATURES_FLAG=""; \
    if [ -n "$CARGO_FEATURES" ]; then FEATURES_FLAG="--features $CARGO_FEATURES"; fi; \
    cargo build $RELEASE_FLAG $NO_DEFAULT_FLAG $FEATURES_FLAG \
        --bin flight-control --package cf-gears-flight-control; \
    cp "/build/target/$OUTPUT_DIR/flight-control" /tmp/flight-control

# ---------------------------------------------------------------------------
# Stage 2: Runtime
# ---------------------------------------------------------------------------
FROM debian:13.7-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Binary (copied via /tmp because the builder target dir is a cache mount).
COPY --from=builder /tmp/flight-control /app/flight-control
# Default config; override by mounting a ConfigMap over /app/config in k8s.
COPY --from=builder /build/config/flight-control.yaml /app/config/flight-control.yaml

# HTTP API / probes (/healthz, /readyz, /health) served on 8087.
EXPOSE 8087

# Runtime state (SQLite DBs, logs) lives under a writable, container-local path
# rather than the non-root user's (absent) home dir. Overrides the config's
# `server.home_dir` via the APP__ env layer. Mount a volume here for persistence.
ENV APP__SERVER__HOME_DIR=/app/data

RUN useradd -U -u 1000 appuser && \
    mkdir -p /app/data && \
    chown -R 1000:1000 /app
USER 1000

CMD ["/app/flight-control", "--config", "/app/config/flight-control.yaml", "run"]
