# Contributing to Project Ember for Linux

Thanks for helping make display control safer on Linux. This project is a port of
the macOS app; behavior should stay aligned with `docs/PORTING.md`.

## Development setup

On Omarchy / Arch:

```bash
sudo pacman -S --needed base-devel rust cargo gtk4 wayland libxkbcommon dbus
git clone https://github.com/mariusschober/ProjectEmberLinux.git
cd ProjectEmberLinux
cargo build --workspace
cargo test --workspace -- --test-threads=1
```

Useful loops:

```bash
./scripts/run-tests.sh
./scripts/red-team.sh
./scripts/e2e-simulated.sh

# GUI visual QA (needs an X11 display; Xvfb is fine)
ember --screenshot /tmp/ember.png --demo --state active
ember --screenshot /tmp/ember-degraded.png --demo --state degraded
```

## Non-negotiable safety rules

Read [`docs/SAFETY.md`](docs/SAFETY.md) before changing any of these:

1. Write the recovery journal **before** mutating a display.
2. Delete a journal entry only after a **verified** restore.
3. Always derive transforms from the immutable baseline.
4. Never blanket-restore on hot-plug.
5. Treat disconnected displays as **pending**, never forgotten.
6. Refuse ambiguous identities instead of guessing.
7. Keep Backlight Lock built-in-only and reversible.
8. Never silently fall back from Wayland to XWayland for color.
9. Report observed truth; do not turn “accepted” into “verified”.

Tests for these invariants live in `crates/ember-core`, `crates/ember-platform`,
`crates/ember-daemon/tests/red_team.rs`, and the simulator scenarios. Add a
regression test with every safety-related fix.

## Code style

- Rust 2021, `cargo fmt --all`, no clippy warnings.
- Keep `ember-core` platform-free; no GTK, Wayland, X11 or D-Bus dependencies.
- Platform calls belong in `ember-platform`; orchestration in `ember-daemon`.
- The GUI is a view: it must not write display state directly.
- User-facing copy should stay factual about verification and limitations.

## Packaging

- `packaging/PKGBUILD` builds the `v$pkgver` tag from GitHub.
- `scripts/package.sh` produces a release source tarball; upload it to the
  matching GitHub release.
- Bump `Cargo.toml`, `ember_core::AppVersion`, `packaging/PKGBUILD`,
  `CHANGELOG.md`, and the release tag together.

## Pull requests

- Describe the user-visible behavior and the safety impact.
- Include the commands you ran and their results.
- Keep unrelated formatting changes out of safety fixes.
- For hardware reports, include `emberctl probe --json`, `emberctl diagnostics
  --export`, compositor/GPU/driver, and the checklist from `docs/OMARCHY.md`.

## License

By contributing, you agree that your contributions are licensed under the MIT
license in [`LICENSE`](LICENSE).
