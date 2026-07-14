#!/bin/sh
# Sentinel agent installer — served by the server at GET /install.sh.
#
#   curl -fsSL https://HOST/install.sh | sudo SENTINEL_TOKEN=xxx sh -s -- --server https://HOST
#   curl -fsSL https://HOST/install.sh | sudo sh -s -- --server https://HOST --token xxx
#
# The SENTINEL_TOKEN env form is preferred: it keeps the enroll token out of
# argv, so it never shows up in `ps` or shell history on the target machine.
set -eu

SERVER="" TOKEN="${SENTINEL_TOKEN:-}" INSECURE_HTTP=0 TOKEN_VIA_ARGV=0
BIN=/usr/local/bin/sentinel-agent

fail() { echo "ERROR: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --server) SERVER="${2:-}"; shift 2 ;;
    --token) TOKEN="${2:-}"; TOKEN_VIA_ARGV=1; shift 2 ;;
    --insecure-http) INSECURE_HTTP=1; shift ;;
    *) fail "unknown argument: $1" ;;
  esac
done

[ -n "$SERVER" ] || fail "--server https://HOST is required"
[ -n "$TOKEN" ] || fail "an enroll token is required (SENTINEL_TOKEN env or --token)"
SERVER="${SERVER%/}"

# Security decision: the enroll token and the downloaded binary must not travel
# in cleartext. Plain http is dev-only and needs an explicit opt-in.
case "$SERVER" in
  https://*) : ;;
  http://*) [ "$INSECURE_HTTP" = 1 ] || fail "refusing plain http:// (use --insecure-http for dev only)" ;;
  *) fail "--server must be an http(s):// URL" ;;
esac

[ "$(id -u)" = 0 ] || fail "must run as root (pipe to: sudo sh -s -- ...)"
[ "$(uname -m)" = x86_64 ] || fail "unsupported architecture $(uname -m) (only x86_64 for now — build from source, see the repo README)"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required (coreutils)"

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1"; }
  fetch_to() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO- "$1"; }
  fetch_to() { wget -qO "$2" "$1"; }
else
  fail "curl or wget is required"
fi

echo "Fetching agent manifest from $SERVER/api/agent/latest ..."
manifest="$(fetch "$SERVER/api/agent/latest")" || fail "could not fetch the agent manifest (does this server bundle an agent build?)"

# Parse with sed (jq may not exist on the target). Strip all whitespace first
# (URLs, hashes and feature names never contain any), then pick the artifact
# object whose features are "headless".
flat="$(printf '%s' "$manifest" | tr -d ' \t\n\r')"
artifact="$(printf '%s' "$flat" | sed -n 's/.*{\([^{}]*"features":"headless"[^{}]*\)}.*/\1/p')"
[ -n "$artifact" ] || fail "no headless artifact in the manifest: $manifest"
url="$(printf '%s' "$artifact" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')"
sha="$(printf '%s' "$artifact" | sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p')"
[ -n "$url" ] && [ -n "$sha" ] || fail "manifest is missing url/sha256: $manifest"
case "$url" in /*) url="$SERVER$url" ;; esac

# Download to a temp file IN THE TARGET DIRECTORY so the final mv is an atomic
# rename on the same filesystem — a crash mid-install can never leave a
# truncated binary at $BIN.
mkdir -p "$(dirname "$BIN")"
tmp="$(dirname "$BIN")/.sentinel-agent.download.$$"
trap 'rm -f "$tmp"' EXIT INT TERM
echo "Downloading $url ..."
fetch_to "$url" "$tmp"

# Security decision: verify the sha256 pinned in the manifest before executing
# anything. Trust model v1 is hash-over-TLS from the enrolled server (see
# docs/CONTRACT-PROD.md); the hash still protects against truncated/corrupted
# downloads and cache tampering.
echo "$sha  $tmp" | sha256sum -c - >/dev/null 2>&1 || fail "sha256 mismatch — refusing to install"

chmod 0755 "$tmp"
mv -f "$tmp" "$BIN"
trap - EXIT INT TERM
echo "Installed $BIN"

echo "Enrolling against $SERVER ..."
"$BIN" enroll --server "$SERVER" --token "$TOKEN"
echo "Installing systemd service ..."
"$BIN" install-service

echo ""
echo "Device enrolled — it should appear online in the console within a minute."
if [ "$TOKEN_VIA_ARGV" = 1 ]; then
  echo "NOTE: the enroll token was passed via --token, so it may linger in this"
  echo "shell's history and was briefly visible in the process list. The token is"
  echo "single-use, but prefer the SENTINEL_TOKEN=... env form next time."
fi
