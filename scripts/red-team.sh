#!/usr/bin/env bash
# Run the adversarial suite plus the protocol and D-Bus boundary tests.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"
echo "== red team =="
cargo test -p ember-daemon --test red_team --locked -- --test-threads=1
echo "== D-Bus boundary =="
cargo test -p ember-daemon --test dbus_integration --locked -- --test-threads=1
echo "== Wayland protocol =="
cargo test -p ember-platform --test wayland_protocol --locked
echo "== filesystem safety =="
cargo test -p ember-core journal --locked -- --test-threads=1
echo "red team: ok"
