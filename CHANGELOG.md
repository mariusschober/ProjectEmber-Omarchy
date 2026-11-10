# Changelog

## 0.4.0 — Omarchy edition (2026-11-10)

First native Linux/Omarchy release. Ported from macOS Project Ember 0.4.0
(build 5) with the same color science and safety model. MIT licensed.

### Added

- Rust workspace: `ember-core`, `ember-platform`, `ember-daemon`, `ember-cli`,
  `ember-gui`.
- Wayland `zwlr_gamma_control_v1` backend (Hyprland/Sway), including
  per-output exclusive control, `failed` handling, and restore-on-destroy.
- X11 XRandR backend with real readback and hot-plug events.
- Built-in-only Backlight Lock via logind → brightnessctl → sysfs, with
  journaled hardware baseline and guard failures.
- Schema-v2 recovery journal with quarantine, size cap, symlink refusal,
  atomic writes, and schema-v1 migration.
- Per-display identity from EDID (UUID/serial/hash), built-in connector rules,
  ambiguity refusal.
- Generation-based topology reconciliation with pending restores and bounded
  external-reset recovery.
- NOAA solar schedule, GeoClue2 optional, manual location, DST and polar
  handling, overrides until the next boundary.
- D-Bus daemon (`org.mariusschober.ProjectEmber`), SNI tray, GTK4 panel with
  optional layer-shell, diagnostics window, CLI (`emberctl`) and GUI (`ember`).
- Omarchy menu extension, keybinding snippet, systemd user unit, D-Bus
  activation, XDG autostart, desktop entry, SVG icons, AppStream metadata.
- PKGBUILD, `scripts/install.sh`, `uninstall.sh`, `package.sh`, test scripts,
  simulated end-to-end script, acceptance probe.

### Changed from macOS

- Wayland verification is "accepted by compositor", not readback; the UI says
  so.
- Automatic-brightness capture is not portable; reported as unavailable.
- Launch at login uses XDG autostart instead of ServiceManagement.
- Tray uses StatusNotifierItem; the panel is a layer-shell overlay on Omarchy.
- Sleep/wake uses logind.
- Nightlight coexistence with `hyprsunset` is detected and user-controlled.

### Fixed during the port

- D-Bus `Quit` now terminates the daemon after restore (watch channel).
- Startup snapshot race in tests.
- GTK slider default highlight and panel fill/footer layout.
- Installer honors `CARGO_TARGET_DIR`.
- X11 `GetCrtcInfo`/gamma-size API usage, serde `displayID` compatibility,
  journal timestamp precision.

### Verified

- 115 core tests, 39 platform tests, mock Wayland protocol test, Xvfb XRandR
  readback test, private D-Bus integration, 7 simulator scenarios, 7 red-team
  tests, GUI screenshots.
- Not yet verified on physical Omarchy hardware; see `docs/OMARCHY.md`.
