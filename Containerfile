# Sentinel — production container image.
#
# Multi-stage build producing a single runtime image containing:
#   - the sentinel-server binary (Rust, from server/ + policy/)
#   - the built web SPA (Bun/Vite, from web/), served by the server itself
#     via SENTINEL_WEB_DIR (see server/src/static_web.rs).
#
# Build from the repo root:
#   podman build -f Containerfile -t sentinel-server:latest .

# ---- Stage 1: Rust builder ------------------------------------------------
# Rust 1.85+ is required: transitive deps (idna_adapter, via url) declare
# edition 2024, which older cargo can't parse. Pinned for reproducible builds.
FROM docker.io/library/rust:1.90-slim-bookworm AS server-builder

# libssl-dev: webauthn-rs pulls openssl (attestation-ca) which links libssl.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# server/ depends on ../policy via a path dependency, so both crates must be
# present in the build context (this Containerfile is NOT a cargo workspace).
COPY policy/ policy/
COPY server/ server/

WORKDIR /build/server
RUN cargo build --release && \
    install -Dm755 target/release/sentinel-server /out/sentinel-server

# ---- Stage 2: Web builder --------------------------------------------------
FROM docker.io/oven/bun:1 AS web-builder

WORKDIR /build/web
COPY web/ .

RUN (bun install --frozen-lockfile || bun install) && bun run build

# ---- Stage 3: Runtime -------------------------------------------------------
FROM docker.io/library/debian:bookworm-slim AS runtime

# libssl3: runtime shared lib for the openssl the server links (see builder note).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 sentinel \
    && useradd --uid 10001 --gid sentinel --no-create-home --shell /usr/sbin/nologin sentinel \
    && mkdir -p /app

COPY --from=server-builder /out/sentinel-server /app/sentinel-server
COPY --from=web-builder /build/web/dist /app/web

RUN chown -R sentinel:sentinel /app

ENV SENTINEL_WEB_DIR=/app/web
ENV BIND_ADDR=0.0.0.0:8080

USER sentinel:sentinel
WORKDIR /app

EXPOSE 8080

ENTRYPOINT ["/app/sentinel-server"]
