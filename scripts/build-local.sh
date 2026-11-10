#!/usr/bin/env bash
# Build release binaries locally. Pass --install to install for the current user.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
cargo build --release --locked --workspace
echo "Built: ${CARGO_TARGET_DIR:-$ROOT/target}/release/{emberd,emberctl,ember}"
if [[ "${1:-}" == "--install" ]]; then
  shift
  exec ./scripts/install.sh --user --no-build "$@"
fi
