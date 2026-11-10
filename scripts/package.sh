#!/usr/bin/env bash
# Build, test and create a release source tarball.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$(awk -F'"' '/^version =/{print $2; exit}' "$ROOT/Cargo.toml")
DIST="$ROOT/dist"
NAME="project-ember-$VERSION"
mkdir -p "$DIST"

echo "Running tests…"
(cd "$ROOT" && cargo test --release --locked --workspace -- --test-threads=1)
echo "Running simulation…"
(cd "$ROOT" && cargo run -q --release -p ember-daemon --bin emberd -- --simulate all)

echo "Creating $DIST/$NAME.tar.gz"
tar -czf "$DIST/$NAME.tar.gz" \
  --exclude='./target' --exclude='./.git' --exclude='./dist' --exclude='./pkg' --exclude='./packaging/*.tar.gz' --exclude='./packaging/src' \
  --transform "s,^\./,$NAME/," \
  -C "$ROOT" .

hash=$(cd "$DIST" && sha256sum "$NAME.tar.gz" | cut -d' ' -f1)
echo "$hash  $NAME.tar.gz" | tee "$DIST/$NAME.tar.gz.sha256"
echo
echo "Release artifact: $DIST/$NAME.tar.gz"
echo "PKGBUILD builds from tag v$VERSION."
echo "Upload with: gh release upload v$VERSION \"$DIST/$NAME.tar.gz\""
