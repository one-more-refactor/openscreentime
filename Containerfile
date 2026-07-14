# Sentinel — production container image.
#
# Multi-stage build producing a single runtime image containing:
#   - the sentinel-server binary (Rust, from server/ + policy/)
#   - the built web SPA (Bun/Vite, from web/), served by the server itself
#     via SENTINEL_WEB_DIR (see server/src/static_web.rs).
#   - the HEADLESS sentinel-agent binary (musl-static, from client/ + policy/)
#     plus its manifest.json under /app/agent, served via SENTINEL_AGENT_DIR
#     (see server/src/agent_dist.rs and GET /install.sh).
#
# Agent build decision: only the default-features (headless) agent is built,
# as a true static musl binary that runs on any x86_64 Linux. The gui/tray
# features (eframe/glow, ksni→libdbus-sys) link C system libraries and can't
# realistically cross-build against musl without a full C sysroot — desktop
# builds come from source (`cargo build --features tray,gui`) until CI exists.
# The headless agent is enforcement-complete; gui/tray only add the lockout
# overlay and the per-user tray companion.
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

# ---- Stage 1b: Agent builder (headless, musl-static) -----------------------
FROM docker.io/library/rust:1.90-slim-bookworm AS agent-builder

# musl-tools: musl-gcc for the crates with C/asm (ring). jq: manifest generation.
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools \
    ca-certificates \
    jq \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl

WORKDIR /build
COPY policy/ policy/
COPY client/ client/

WORKDIR /build/client
RUN cargo build --release --target x86_64-unknown-linux-musl

# Stage the versioned artifact + manifest.json for /api/agent/latest.
# jq builds the JSON so a weird value can never produce a malformed manifest.
RUN set -eu; \
    VERSION="$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "sentinel-agent") | .version')"; \
    FILE="sentinel-agent-${VERSION}-x86_64-musl"; \
    mkdir -p /out/agent; \
    install -m 0755 "target/x86_64-unknown-linux-musl/release/sentinel-agent" "/out/agent/${FILE}"; \
    SHA256="$(sha256sum "/out/agent/${FILE}" | cut -d' ' -f1)"; \
    jq -n --arg version "$VERSION" --arg file "$FILE" --arg sha256 "$SHA256" \
        '{version: $version, artifacts: [{target: "x86_64-linux-musl", features: "headless", url: ("/api/agent/download/" + $file), sha256: $sha256}]}' \
        > /out/agent/manifest.json; \
    cat /out/agent/manifest.json

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
COPY --from=agent-builder /out/agent /app/agent

RUN chown -R sentinel:sentinel /app

ENV SENTINEL_WEB_DIR=/app/web
ENV SENTINEL_AGENT_DIR=/app/agent
ENV BIND_ADDR=0.0.0.0:8080

USER sentinel:sentinel
WORKDIR /app

EXPOSE 8080

ENTRYPOINT ["/app/sentinel-server"]
