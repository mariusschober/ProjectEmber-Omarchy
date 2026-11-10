# Test plan and evidence

Last run: 2026-11-08/09, Linux aarch64 container (Ubuntu 24.04 userland,
Rust 1.99.1), GTK 4.14.5 from a local sysroot, Xvfb 21.1.

## Automated

| Layer | Command | Result |
|---|---|---|
| Core unit tests | `cargo test -p ember-core` | 115 passed |
| Platform unit tests | `cargo test -p ember-platform --lib` | 39 passed |
| Wayland protocol (mock compositor) | `cargo test -p ember-platform --test wayland_protocol` | 1 passed |
| X11 XRandR readback (Xvfb) | `DISPLAY=:99 cargo test -p ember-platform --test x11_gamma` | 1 passed |
| Daemon simulation | `cargo run -p ember-daemon -- --simulate all` | 7 scenarios passed |
| D-Bus integration | `cargo test -p ember-daemon --test dbus_integration` | 1 passed |
| Red team | `cargo test -p ember-daemon --test red_team` | 7 passed |
| GUI render | `ember --screenshot … --demo --state {active,off,degraded}` | 3 images, visually inspected |
| Live GUI + daemon + CLI | private `dbus-run-session`, simulated backend | screenshot matched daemon state |
| Workspace build | `cargo build --workspace` | clean (warnings triaged) |
| Release build | `cargo build --release --locked --workspace` | passed |
| x86_64 cross-check | `cargo check -p ember-core -p ember-platform -p ember-daemon -p ember-cli --target x86_64-unknown-linux-gnu` | passed |

### Exact commands

```sh
# from the repository root
. /path/to/env.sh                  # toolchain paths (local dev container)
cargo test --workspace -- --test-threads=1
cargo run -q -p ember-daemon --bin emberd -- --simulate all

# X11 (Xvfb)
Xvfb :99 -screen 0 1920x1200x24 &
DISPLAY=:99 cargo test -p ember-platform --test x11_gamma -- --nocapture

# GUI screenshots
GDK_BACKEND=x11 DISPLAY=:99 GSK_RENDERER=cairo \
  cargo run -p ember-gui -- --screenshot /tmp/panel.png --demo --state active

# Full drive-through
dbus-run-session -- bash scripts/e2e-simulated.sh
```

## Scenarios (`emberctl simulate all`)

| Scenario | Asserts |
|---|---|
| `basic` | activation, verification, warmth/brightness, presets, off, journal cleared |
| `hotplug` | unplug retains pending; replug restores that display; survivor untouched |
| `crash-recovery` | journaled state restored after simulated crash; starts off |
| `corrupt-journal` | quarantine, no mutation, truthful attention |
| `polar-solar` | polar night: no sunrise event; schedule remains sane |
| `backlight` | built-in-only lock, baseline capture, restore, failure guard |
| `external-reset` | external LUT reset re-applied; bounded escalation |

Each scenario returns a JSON report with per-step evidence; the process exits
non-zero if any scenario fails.

## Protocol evidence (Wayland)

The mock compositor in `crates/ember-platform/tests/wayland_protocol.rs`:

- advertises `wl_output` (name `eDP-1`, built-in) and
  `zwlr_gamma_control_manager_v1`;
- records the fd contents of `set_gamma` and compares them **exactly** with
  `GammaTable::to_u16_blocks(256)` after a warmth/brightness transform;
- verifies `capture_baseline` reports `CompositorDefault`;
- verifies `verify` returns `AcceptedUnverified` (no fake readback);
- verifies `restore` sends `destroy` and the compositor clears the table;
- a second server sends `failed`, and the backend then reports gamma size 0,
  an unsupported reason, and refuses baseline/apply.

This is the strongest available proof without Hyprland hardware.

## X11 evidence

Xvfb exposes a `screen` output with a writable 256-entry LUT. The test:

1. connects through XRandR and asserts `VerificationLevel::Readback`;
2. captures a baseline with `GammaSource::Readback`;
3. applies a warmth table and asserts `report.verified`;
4. reads it back and asserts equality;
5. restores the baseline and reads it back;
6. applies a second different table and restores again, proving transforms do
   not compound.

## D-Bus evidence

`crates/ember-daemon/tests/dbus_integration.rs` starts a private
`dbus-daemon`, serves the real interface, then:

- waits for coordinator startup;
- enables Ember and observes `observedActive` with 2 verified displays;
- sets warmth/brightness, applies Pure Red;
- asserts NaN warmth, 0.02 brightness, and an unknown preset are rejected;
- exports diagnostics and checks it identifies itself and contains no secrets;
- disables and waits for pending=0 and journal deletion;
- attempts a second daemon (single-instance boundary).

## Red-team evidence

`crates/ember-daemon/tests/red_team.rs`:

| Test | Attack | Result |
|---|---|---|
| `single_instance_lock_is_exclusive` | concurrent daemons | second refused; lock released on drop |
| `future_schema_is_quarantined_and_no_display_is_mutated` | schema 99 | preserved, attention, zero mutation |
| `corrupted_journal_is_quarantined_not_deleted` | binary garbage | bytes preserved in corrupt file |
| `restore_failure_keeps_journal_and_reports_degraded` | failed restore | degraded, journal retained |
| `pending_display_is_restored_on_reconnect_without_blanking_survivor` | unplug/replug | pending restored; survivor unchanged |
| `oversized_journal_is_rejected_without_allocation` | 2 MiB journal | rejected as corrupt |
| `journal_never_resolves_to_a_different_display` | same connector, foreign identity | stays pending; baseline untouched |

## GUI evidence

Rendered with the app's own `--screenshot` path (WidgetPaintable → render node →
texture → PNG), under Xvfb with the Cairo renderer and the local GTK 4.14.5
sysroot:

- `design/screenshots/panel-active.png` — active, Evening, 2 displays;
- `design/screenshots/panel-off.png` — ready, original state untouched;
- `design/screenshots/panel-degraded.png` — attention + Retry;
- `design/screenshots/panel-live-simulated.png` — live snapshot from `emberd`
  over D-Bus after `emberctl on` (not demo data).

Inspected for: clipped text, contrast, slider gradients (default GTK blue
highlight removed), footer link legibility, window height, switch states, orb
state, preset selection.

## Manual checks performed

- `emberd --simulate all` exits 0 and prints `"passed": true`.
- `emberctl status`, `on`, `off`, `warmth`, `brightness`, `preset`,
  `diagnostics`, `quit` against a simulated daemon.
- `emberctl quit` now causes the process to exit after restore; the earlier
  version waited for a signal forever (fixed by a `watch` shutdown channel).
- GUI polling reflects daemon state within ~220 ms; no writes originate in the
  GUI.
- Install/uninstall dry run and prefix install (see `scripts/run-tests.sh`).
- `desktop-file-validate` where available; service/desktop Exec paths rewritten
  for custom prefixes.

## Not testable in this container (hardware acceptance required)

- Real Hyprland `zwlr_gamma_control_v1` semantics (exclusive control, failed
  events, compositor restore).
- Real EDID/connector names and GPU LUT precision/banding.
- Actual panel backlight via logind/`brightnessctl`/sysfs.
- Real sleep/wake.
- Omarchy Quickshell tray rendering and layer-shell placement.
- HiDPI scaling and multi-monitor mixed DPI.
- Intel MacBook backlight device naming and T2 quirks.

These are the explicit gates in `docs/OMARCHY.md` before hardware sign-off.

## Portability matrix

| Platform | Backend | Expected |
|---|---|---|
| Omarchy / Hyprland (Wayland) | wlr-gamma-control | full, accepted verification |
| Sway / wlroots | wlr-gamma-control | full, accepted verification |
| Other Wayland compositors without wlr protocol | none | truthful "no compatible display", no mutation |
| X11 session | XRandR | full, readback verification |
| XWayland under Hyprland | not used | Wayland path selected |
| Headless / no session | null | daemon and UI run; no mutation |
| x86_64 and aarch64 | all Rust code | same behavior; release build per arch |
