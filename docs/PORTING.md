# macOS → Linux port analysis

This document records the review of `/project-ember` (macOS 0.4.0, Swift) and
how each idea maps to the Linux/Omarchy implementation in this repository.

## What the macOS app is

- Native AppKit menu-bar app, ~10,000 lines including tests, macOS 14+, arm64.
- `EmberCore` (platform-free) contains the color science, gamma tables, display
  identity, topology/reconciler model, state machine, presentation model,
  settings and recovery journal.
- `ProjectEmber` (platform) talks to CoreGraphics display transfer tables,
  private DisplayServices for Backlight Lock, Core Location, ServiceManagement
  and AppKit.
- A custom-drawn 390 pt dark panel: header, hero orb (on/off), preset pill,
  warmth/brightness sliders, Backlight Lock / Sun schedule / Launch at login /
  menu-bar behaviour rows, diagnostics, footer.
- Journal-before-mutation, immutable baselines, verified restores, per-display
  identity, generation-based reconciliation, no blanket restore on hot-plug,
  disconnected displays retained as pending, corrupt journals quarantined,
  no network or telemetry.

The Linux port preserves those **safety semantics** and the color math, not the
AppKit code. The macOS sources were used as the specification.

## Feature mapping

| macOS | Linux / Omarchy | Notes |
|---|---|---|
| `CGSetDisplayTransferByTable` | `zwlr_gamma_control_v1` (Hyprland/Sway) or XRandR `set_crtc_gamma` | Wayland has no readback; X11 does |
| `CGGetDisplayTransferByTable` baseline | X11 readback; Wayland: compositor default + `destroy` restores | Recorded as `gammaSource` |
| `CGDirectDisplayID` + UUID | EDID-derived UUID + connector + vendor/model/serial/unit | Connector names like `eDP-1`, `DP-1` |
| Display reconfiguration observer | Wayland registry + `wl_output` events; RandR events on X11 | Debounced generations |
| DisplayServices Backlight Lock | systemd-logind `SetBrightness` → `brightnessctl` → sysfs | Built-in only; verified |
| Automatic-brightness capture | **Not controllable** on Linux; sensor presence reported | Honest capability limit |
| Core Location one-shot | GeoClue2 (optional) + manual rounded coordinates | Ember does no network |
| ServiceManagement launch at login | XDG autostart entry | Systemd unit also available |
| `NSStatusItem` | StatusNotifierItem tray (ksni) | Omarchy bar has an SNI tray |
| AppKit panel | GTK4 panel + optional `gtk4-layer-shell` | Anchored under the Omarchy bar |
| Menu-bar click behaviour | Tray click behaviour setting | Open Controls / Toggle |
| `NSApplication` sleep/wake notifications | logind `PrepareForSleep` | Restore before sleep |
| Solar NOAA calculation | Same algorithm ported to `chrono` | DST and polar tests |
| JSON journal schema v2 | Same shape, camelCase, epoch seconds; v1 read | 1 MiB cap, 0600, quarantine |
| UserDefaults settings | JSON in `$XDG_CONFIG_HOME/project-ember` | Atomic, 0600, normalized |
| ColorSync force reset fallback | Wayland `destroy` / compositor reset; X11 cannot force | Explicitly reported |

## Color pipeline

The math was ported line-for-line and tested against the same expected values:

- warmth `0..0.82` → smoothstep → 6500 K..2000 K CIE xy → linear sRGB gains;
- final `0.18` → red-only Pure Red;
- software brightness multiplies the transformed channels, clamped to 10–100 %;
- every transform derives from the immutable baseline, so slider moves never
  compound.

## Identity and ambiguity

Linux exposes no CoreGraphics UUID. The port derives identity from EDID
(manufacturer/product/serial, or a hash of the EDID bytes when no serial
exists) and the connector name. Internal panels are `eDP-*`, `LVDS-*`, `DSI-*`,
`DPI-*`. An external display with neither EDID nor serial is deliberately
**ambiguous** and is left untouched — the same no-first-match rule as macOS.
Duplicate online identities are treated as ambiguous and never mutated.

## Deliberate differences

1. **Wayland verification level.** `zwlr_gamma_control_v1` has no getter. Ember
   verifies that the compositor accepted each ramp and that the control object
   was not failed; the UI/diagnostics say "Accepted by compositor", not "read
   back". X11 keeps true readback.
2. **Backlight Lock scope.** Only the built-in panel, only one unambiguous
   `/sys/class/backlight` device, and no ambient-light automation. External
   DDC/CI is explicitly out of scope, matching macOS.
3. **Nightlight coexistence.** Omarchy's `hyprsunset` uses Hyprland's CTM
   protocol, which is separate from gamma control. Ember therefore can coexist
   technically, but the effects compound. The tray/GUI reports when nightlight
   is active and offers a user-initiated "turn off" action; Ember never changes
   another app's state on its own.
4. **No color-profile manager.** Hyprland refuses `set_gamma` when ICC ramps are
   in use; Ember surfaces the failure instead of silently failing.
5. **Tray vs menu bar.** A StatusNotifierItem cannot draw a custom popover
   anchored to the icon; the GTK panel is a layer-shell overlay under the bar
   on Omarchy, with a normal-window fallback elsewhere.
6. **Sleep/wake** uses logind instead of AppKit notifications.

## What was intentionally not ported

- Screen-recording, accessibility, camera, microphone or network permissions
  (none are needed).
- Developer ID/notarization and DMG creation (Arch packaging instead).
- CoreGraphics-specific ColorSync fallback and private DisplayServices.
- macOS menu-bar drawing details that do not map to GTK; the visual language is
  recreated with GTK/Cairo and CSS.

## Verification of the port

The Linux core was tested against the same behavioral expectations as the
macOS suite (warmth/kelvin values, journal corruption, duplicate identities,
unplug survivor behavior, state machine transitions, solar DST/polar). On top
of that:

- a mock Wayland compositor asserts exact u16 ramp bytes and object lifetime;
- XRandR readback is exercised against Xvfb;
- the daemon is driven over a private D-Bus bus;
- the GUI is rendered under Xvfb and inspected.

See `TEST_PLAN.md` and `RED_TEAM_REPORT.md`.
