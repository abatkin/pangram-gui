#!/usr/bin/env bash
# UI smoke test against the local mock API. Uses throwaway settings/history directories and an
# in-memory key store, so it never touches real data, the system keyring, or Pangram credits.
#
#   scripts/ui-smoke.sh [screenshot-dir] [extra app args, e.g. --smoke-dark]
#
# Set QT_QPA_PLATFORM=offscreen to run without showing a window (needed for screenshots).
# On Wayland the clipboard checks only pass if the window has keyboard focus.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-}"
shift || true
tmp="$(mktemp -d)"
trap 'kill "${mock_pid:-}" 2>/dev/null || true; rm -rf "$tmp"' EXIT

cargo build -q -p pangram-desktop
cargo build -q -p pangram-core --example mock_server

port=$((20000 + RANDOM % 20000))
MOCK_PORT=$port MOCK_DELAY_MS=1500 target/debug/examples/mock_server >"$tmp/mock.log" &
mock_pid=$!
for _ in $(seq 50); do grep -q "Mock Pangram API" "$tmp/mock.log" && break; sleep 0.1; done

args=(--qml-script="$PWD/crates/pangram-desktop/tests/ui/smoke.qml")
if [[ -n "$out" ]]; then
  mkdir -p "$out"
  args+=(--smoke-out="$(realpath "$out")")
fi

XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" \
  PANGRAM_API_BASE="http://127.0.0.1:$port" PANGRAM_CREDENTIAL_STORE=memory \
  QT_FORCE_STDERR_LOGGING=1 \
  target/debug/pangram-desktop "${args[@]}" "$@"
