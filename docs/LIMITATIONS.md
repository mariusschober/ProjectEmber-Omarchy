# Limitations

These are deliberate boundaries or platform facts, not TODOs disguised as
features. The UI and diagnostics state them truthfully.

## Display pipeline

- **Wayland has no gamma readback.** `zwlr_gamma_control_v1` has no getter. Ember
  confirms the compositor accepted the fd and that the control was not failed;
  it cannot prove the GPU produced the exact requested values. The UI says
  "Accepted by compositor", never "verified by readback". X11 does read back.
- **One gamma client per output.** Hyprland refuses a second
  `zwlr_gamma_control_v1` for an output (`failed`). Another night-light/gamma
  tool that holds the protocol will make Ember report unsupported; Ember does
  not fight it.
- **ICC/gamma ramps in use.** Hyprland rejects `set_gamma` when
  `gamma_ramps_in_use` is set (e.g. an ICC profile is active). Ember surfaces
  the failure; disable the conflicting color management if you want Ember's
  LUT.
- **Compositor-wide effect.** Gamma applies to the whole output, including the
  Ember panel and lock screen, not just app windows.
- **XWayland is not used as a fallback.** Under a Wayland session Ember uses
  wlr-gamma-control or reports unsupported. XWayland gamma would only affect
  X11 clients and would be misleading.
- **X11 precision depends on the driver/CRTC.** Xvfb and common Intel/AMD
  drivers expose 256–4096 entry LUTs; Ember resamples to the available size.
  Extreme low brightness can show banding on 8-bit panels; this is a hardware
  limit.
- **No KMS/DRM direct writes.** Ember never becomes root, never opens
  `/dev/dri/card*`, and never bypasses the compositor. This is intentional.

## Backlight Lock

- Built-in panels only (`eDP-*`, `LVDS-*`, `DSI-*`, `DPI-*`). External DDC/CI is
  not implemented (matching macOS).
- One unambiguous `/sys/class/backlight` device. Multiple devices or unknown
  names → unavailable, never a guess.
- No ambient-light / auto-brightness automation. Linux does not expose a
  portable ambient control; Ember reports whether a sensor exists rather than
  pretending.
- A power loss or SIGKILL while locked can leave the panel at full hardware
  brightness until Ember restarts and restores the journal. A userspace daemon
  cannot eliminate this window.
- Kernel/polkit policy can deny logind brightness calls; the fallbacks may also
  be denied. Ember reports failure and does not loop.

## Sun schedule

- Coordinates are approximate and rounded to 0.1°; this is a lighting schedule,
  not navigation.
- GeoClue may be unavailable, denied, or configured to use network services.
  Ember itself performs no network request; manual coordinates always work.
- Polar day/night has no sunrise/sunset event by definition; Ember leaves the
  schedule in a truthful no-event state.
- Clock/timezone jumps are handled by recalculation, but a suspended machine
  can miss an exact boundary; the next evaluation applies the correct state.

## Desktop integration

- The tray is StatusNotifierItem, not a macOS-style popover anchored to the
  icon. On Omarchy the GTK panel uses `gtk4-layer-shell` to sit under the bar;
  elsewhere it is a normal window.
- GNOME/KDE are not primary targets; they may not implement
  `wlr-gamma-control`. KDE's own night light is separate.
- No global keyboard shortcut is installed automatically; use Omarchy's Lua
  bindings or your desktop's shortcut settings.
- The Omarchy menu extension is opt-in and writes only a separate JSONC file;
  Ember does not edit `hyprland.lua`, `bindings.lua`, or the bar config.
- `hyprsunset` uses CTM; it can coexist with Ember but the tints compound.
  Ember detects and reports it and offers a manual turn-off.

## Security / trust

- The D-Bus API is on the user's session bus. Any same-UID process can call it;
  that is equivalent to controlling the session's displays.
- Root, a compromised compositor, a malicious GPU driver, or a kernel bug are
  outside the threat model.
- The PKGBUILD pins the hash of the included source tarball. Regenerate the
  tarball and update `sha256sums` (and `.SRCINFO`) for every release; do not
  accept a mismatched tarball.
- Project Ember is MIT licensed; see `LICENSE`.

## Not yet hardware-tested

The implementation is simulated and protocol-tested, but as of this build it has
not run on a physical Omarchy machine. Specifically unverified: real Hyprland
LUT behavior, Intel MacBook backlight naming/T2 quirks, Quickshell tray/layer
placement, mixed-DPI multi-monitor, and suspend/resume. `docs/OMARCHY.md`
contains the acceptance checklist that closes these gaps.
