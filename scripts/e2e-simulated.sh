#!/usr/bin/env bash
# Drive the real daemon, CLI and (if a display exists) GUI on a private bus.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/debug"
if [[ ! -x "$BIN/emberd" ]]; then
  cargo build -q -p ember-daemon -p ember-cli -p ember-gui
fi
tmp=$(mktemp -d)
export EMBER_BACKEND=sim
export EMBER_CONFIG_DIR="$tmp/config"
export EMBER_STATE_DIR="$tmp/state"
export EMBER_RUNTIME_DIR="$tmp/run"
export XDG_RUNTIME_DIR="$tmp/runtime"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

cleanup() {
  "$BIN/emberctl" quit >/dev/null 2>&1 || true
  [[ -n "${DAEMON:-}" ]] && kill "$DAEMON" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

"$BIN/emberd" --no-tray >"$tmp/daemon.log" 2>&1 &
DAEMON=$!
for _ in $(seq 1 50); do
  if "$BIN/emberctl" status >/dev/null 2>&1; then break; fi
  sleep 0.1
done

echo "-- status --"
"$BIN/emberctl" status
echo "-- on --"
"$BIN/emberctl" on
echo "-- set warmth/brightness --"
"$BIN/emberctl" warmth 72 >/dev/null
"$BIN/emberctl" brightness 60 >/dev/null
"$BIN/emberctl" preset evening >/dev/null
"$BIN/emberctl" status --json | head -c 400; echo
if [[ -n "${DISPLAY:-}" ]]; then
  echo "-- live GUI screenshot --"
  "$BIN/ember" --screenshot "$tmp/live.png"
fi
echo "-- off --"
"$BIN/emberctl" off
echo "-- quit --"
"$BIN/emberctl" quit
sleep 1
if kill -0 "$DAEMON" 2>/dev/null; then
  echo "daemon did not exit after quit" >&2
  exit 1
fi
DAEMON=
echo "e2e simulated: ok"
