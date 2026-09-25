# OpenScreenTime — production container image.
#
# Multi-stage build producing a single runtime image containing:
#   - the openscreentime-server binary (Rust, from server/ + policy/)
#   - the built web SPA (Bun/Vite, from web/), served by the server itself
#     via OST_WEB_DIR (see server/src/static_web.rs).
#   - TWO openscreentime binaries under /app/agent, plus a two-artifact
#     manifest.json, served via OST_AGENT_DIR (see server/src/agent_dist.rs
#     and GET /install.sh):
#       * headless — musl-static, runs on ANY x86_64 Linux (servers, minimal
#         installs). Enforcement-complete, no user-facing surface.
#       * desktop  — glibc-dynamic, built with --features gui,tray. Adds the
#         fullscreen lockout overlay and the per-user tray (the time meter,
#         notifications, "ask for more time"). install.sh installs this one on
#         a machine with a graphical session — i.e. the managed child laptop.
#
# The desktop build can't be static-musl (eframe/glow and ksni→libdbus-sys link
# C system libraries), so it ships glibc-dynamic against the runtime libs any
# GNOME/KDE install already has. Both are x86_64 only for now.
#
# Build from the repo root:
#   podman build -f Containerfile -t openscreentime-server:latest .

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
    install -Dm755 target/release/openscreentime-server /out/openscreentime-server

# ---- Stage 1b: Agent builder (headless musl-static + desktop glibc) --------
FROM docker.io/library/rust:1.90-slim-bookworm AS agent-builder

# musl-tools: musl-gcc for the crates with C/asm (ring). jq: manifest generation.
# The -dev packages are for the desktop build's GUI (eframe/glow → OpenGL/xkb/
# wayland) and tray (notify-rust → dbus); they're only needed to LINK — the
# desktop binary loads their runtime .so counterparts on the target.
RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools \
    ca-certificates \
    jq \
    pkg-config libdbus-1-dev libxkbcommon-dev libwayland-dev \
    libxcb1-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl

WORKDIR /build
COPY policy/ policy/
COPY client/ client/
# The agent draws its icons from the brand set (client/src/icons.rs and
# service.rs include_str! ../../brand/icons/*.svg and ../../brand/app-icon*.svg),
# so those ride along — without them the agent build fails.
COPY brand/icons/ brand/icons/
COPY brand/app-icon*.svg brand/

WORKDIR /build/client
# The build id: a hash of the agent's source (client/ + policy/ + the brand
# files it embeds). Compiled into
# the binaries and published in the manifest, so an installed agent can tell
# "a different build" from "a different version number" — a fix merged
# without a version bump still reaches the fleet, and an unchanged agent is
# not re-installed just because the server image was rebuilt. CI computes it
# the same way (.github/workflows/build.yml).
RUN cd /build && find policy client brand/icons brand/app-icon*.svg -type f -not -path '*/target/*' -print0 \
        | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum | cut -c1-16 > /build/agent-build-id \
    && cat /build/agent-build-id
RUN OST_BUILD_ID="$(cat /build/agent-build-id)" \
    cargo build --release --target x86_64-unknown-linux-musl
RUN OST_BUILD_ID="$(cat /build/agent-build-id)" \
    cargo build --release --features gui,tray

# Stage both versioned artifacts + a two-artifact manifest.json for
# /api/agent/latest. jq builds the JSON so a weird value can never produce a
# malformed manifest. Built on bookworm, the desktop binary needs glibc 2.35
# at most (checked in CI), so it runs on Debian 12, Ubuntu 22.04 and newer.
RUN set -eu; \
    VERSION="$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "openscreentime") | .version')"; \
    BUILD="$(cat /build/agent-build-id)"; \
    mkdir -p /out/agent; \
    FILE="openscreentime-${VERSION}-x86_64-musl"; \
    install -m 0755 "target/x86_64-unknown-linux-musl/release/openscreentime" "/out/agent/${FILE}"; \
    SHA256="$(sha256sum "/out/agent/${FILE}" | cut -d' ' -f1)"; \
    DFILE="openscreentime-${VERSION}-x86_64-desktop"; \
    install -m 0755 "target/release/openscreentime" "/out/agent/${DFILE}"; \
    DSHA256="$(sha256sum "/out/agent/${DFILE}" | cut -d' ' -f1)"; \
    jq -n --arg version "$VERSION" --arg build "$BUILD" --arg file "$FILE" --arg sha256 "$SHA256" --arg dfile "$DFILE" --arg dsha256 "$DSHA256" '{version: $version, build: $build, artifacts: [{target: "x86_64-linux-musl", features: "headless", url: ("/api/agent/download/" + $file), sha256: $sha256}, {target: "x86_64-linux-gnu", features: "desktop", url: ("/api/agent/download/" + $dfile), sha256: $dsha256}]}' > /out/agent/manifest.json; \
    cat /out/agent/manifest.json

# ---- Stage 2: Web builder --------------------------------------------------
FROM docker.io/oven/bun:1 AS web-builder

WORKDIR /build/web
# The console draws its icons straight from the brand set (web/src/components/Icon.tsx
# imports ../../../brand/icons/*.svg), so that folder rides along. `tsc -b` also
# type-checks the tests, which share the schedule vectors with the policy crate
# (web/src/lib/{schedule,myRules}.test.ts import ../../../policy/tests/*.json).
COPY brand/icons/ /build/brand/icons/
COPY policy/tests/schedule-vectors.json /build/policy/tests/schedule-vectors.json
COPY web/ .

RUN (bun install --frozen-lockfile || bun install) && bun run build

# ---- Stage 3: Runtime -------------------------------------------------------
FROM docker.io/library/debian:bookworm-slim AS runtime

# libssl3: runtime shared lib for the openssl the server links (see builder note).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 openscreentime \
    && useradd --uid 10001 --gid openscreentime --no-create-home --shell /usr/sbin/nologin openscreentime \
    && mkdir -p /app

COPY --from=server-builder /out/openscreentime-server /app/openscreentime-server
COPY --from=web-builder /build/web/dist /app/web
COPY --from=agent-builder /out/agent /app/agent

RUN chown -R openscreentime:openscreentime /app

ENV OST_WEB_DIR=/app/web
ENV OST_AGENT_DIR=/app/agent
ENV BIND_ADDR=0.0.0.0:8080

USER openscreentime:openscreentime
WORKDIR /app

EXPOSE 8080

ENTRYPOINT ["/app/openscreentime-server"]
