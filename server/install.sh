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
BIN=/usr/local/bin/openscreentime
# Which build to install. "auto" picks the desktop (gui+tray) artifact on a
# machine that has a graphical session and falls back to headless everywhere
# else; --headless / --desktop force it.
VARIANT=auto

fail() { echo "ERROR: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --server) SERVER="${2:-}"; shift 2 ;;
    --token) TOKEN="${2:-}"; TOKEN_VIA_ARGV=1; shift 2 ;;
    --insecure-http) INSECURE_HTTP=1; shift ;;
    --headless) VARIANT=headless; shift ;;
    --desktop) VARIANT=desktop; shift ;;
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

# In auto mode, install the desktop build only where it can actually show its
# UI: a real graphical session. A headless server that happens to have Xorg
# libraries installed still wants the static musl binary, so key off a live
# session (loginctl seat with graphical type, or a set DISPLAY/WAYLAND socket),
# not merely the presence of the libs.
detect_graphical() {
  [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && return 0
  command -v loginctl >/dev/null 2>&1 || return 1
  # Iterate with `for` over a command substitution — NOT `... | while`, whose
  # subshell can't return from this function and would mis-report either way.
  for s in $(loginctl list-sessions --no-legend 2>/dev/null | awk '{print $1}'); do
    t="$(loginctl show-session "$s" -p Type --value 2>/dev/null)"
    case "$t" in wayland|x11) return 0 ;; esac
  done
  return 1
}

want="$VARIANT"
if [ "$want" = auto ]; then
  if detect_graphical; then want=desktop; else want=headless; fi
fi

# Parse with sed (jq may not exist on the target). Strip all whitespace first
# (URLs, hashes and feature names never contain any). Pick the artifact whose
# features match what we want; if the desktop build isn't in the manifest (an
# older server), fall back to headless rather than failing the install.
flat="$(printf '%s' "$manifest" | tr -d ' \t\n\r')"
pick() { printf '%s' "$flat" | sed -n "s/.*{\\([^{}]*\"features\":\"$1\"[^{}]*\\)}.*/\\1/p"; }
artifact="$(pick "$want")"
if [ -z "$artifact" ] && [ "$want" = desktop ]; then
  echo "No desktop artifact in the manifest; falling back to the headless build (no tray/overlay)." >&2
  want=headless
  artifact="$(pick headless)"
fi
[ -n "$artifact" ] || fail "no '$want' artifact in the manifest: $manifest"
echo "Selected the '$want' agent build."
url="$(printf '%s' "$artifact" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')"
sha="$(printf '%s' "$artifact" | sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p')"
[ -n "$url" ] && [ -n "$sha" ] || fail "manifest is missing url/sha256: $manifest"
case "$url" in /*) url="$SERVER$url" ;; esac

# Download to a temp file IN THE TARGET DIRECTORY so the final mv is an atomic
# rename on the same filesystem — a crash mid-install can never leave a
# truncated binary at $BIN.
mkdir -p "$(dirname "$BIN")"
tmp="$(dirname "$BIN")/.openscreentime.download.$$"
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
