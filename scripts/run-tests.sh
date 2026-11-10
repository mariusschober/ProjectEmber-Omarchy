#!/usr/bin/env bash
# Full local test suite. Requires a Rust toolchain and (for X11/GUI) a display.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

echo "== cargo test (workspace, serial) =="
cargo test --workspace --locked -- --test-threads=1

echo "== daemon simulation =="
cargo run -q -p ember-daemon --bin emberd -- --simulate all

if [[ -n "${DISPLAY:-}" ]]; then
  echo "== X11 XRandR test (DISPLAY=$DISPLAY) =="
  cargo test -p ember-platform --test x11_gamma --locked -- --nocapture
else
  echo "== X11 test skipped (no DISPLAY) =="
fi

if [[ -n "${DISPLAY:-}" && -x "$TARGET_DIR/debug/ember" ]]; then
  echo "== GUI screenshot =="
  mkdir -p design/screenshots
  "$TARGET_DIR/debug/ember" --screenshot design/screenshots/panel-active.png --demo --state active
  "$TARGET_DIR/debug/ember" --screenshot design/screenshots/panel-off.png --demo --state off
  "$TARGET_DIR/debug/ember" --screenshot design/screenshots/panel-degraded.png --demo --state degraded
else
  echo "== GUI screenshot skipped (no DISPLAY or ember binary) =="
fi

echo "== installer dry run =="
tmp_home=$(mktemp -d)
HOME="$tmp_home" CARGO_TARGET_DIR="$TARGET_DIR" ./scripts/install.sh --user --no-build --dry-run >/dev/null
rm -rf "$tmp_home"
echo "installer dry run ok"

echo "== release build =="
cargo build --release --locked --workspace

echo
echo "All automated checks passed."
