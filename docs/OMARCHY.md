# Omarchy integration and hardware acceptance

Target: **Omarchy 3.8.5 “quattro”** (Arch Linux, Hyprland, Quickshell
`omarchy-shell`), x86_64 and aarch64. The same binaries also run on Sway and
other wlroots compositors, and on plain X11 desktops.

## What Omarchy provides that Ember uses

| Component | Use |
|---|---|
| Hyprland `zwlr_gamma_control_unstable_v1` | primary gamma path |
| Quickshell bar + StatusNotifierItem tray (`omarchy.tray`) | tray icon, menu |
| `gtk4` (4.14+) | control panel |
| `gtk4-layer-shell` | anchored overlay under the bar |
| `brightnessctl`, `systemd-logind` | Backlight Lock fallbacks |
| `/sys/class/backlight` | hardware backlight truth |
| `hyprctl` | read-only monitor/mirroring/nightlight detection |
| `hyprsunset` | **separate** CTM-based nightlight; reported, not modified |
| `omarchy` menu (Quickshell JSONC extensions) | optional Ember menu rows |
| Lua Hyprland bindings (`o.bind`) | optional keybindings |
| systemd user services / XDG autostart | launch at login |

Ember never edits `~/.config/hypr/*`, the Omarchy config, or another app's
state. Integration is via standard XDG/SNI/D-Bus and opt-in files.

## Install on Omarchy

```sh
cd packaging
makepkg -si
systemctl --user daemon-reload
systemctl --user enable --now project-ember.service
```

or:

```sh
./scripts/install.sh --omarchy
```

The tray icon appears in the Omarchy bar. If your bar hides SNI items, enable
the Ember entry in the bar settings; Ember itself does not modify the bar
config.

### Menu extension

Omarchy reads a single extension file:
`~/.config/omarchy/extensions/omarchy-menu.jsonc`. `--omarchy` installs the
Ember snippet there **only if the file does not exist**. If it exists, the
installer never overwrites or edits it; the snippet is written to
`~/.local/share/doc/project-ember/omarchy-menu.jsonc` (or the prefix doc
directory) and the installer prints a merge instruction.

The snippet adds **Ember → Ember Controls / Toggle Ember / Diagnostics /
Restore Displays / Toggle Sun Schedule**. It uses the documented JSONC schema
(`id`, `icon`, `label`, `action`, `description`); the icon is a Nerd Font
glyph, so it matches the rest of the menu. Merge the object entries inside the
snippet into your existing file, then run `omarchy menu refresh` (or reopen the
menu).

### Keybindings

Add to `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + SHIFT + E", "Ember controls", "ember")
o.bind("SUPER + SHIFT + T", "Toggle Ember", "emberctl toggle")
o.bind("SUPER + SHIFT + N", "Ember night preset", "emberctl preset evening")
```

Then `hyprctl reload`. `install.sh --omarchy` copies this snippet to
`~/.local/share/doc/project-ember/hypr-bindings.lua` for reference; it does not
edit your bindings.

## Nightlight interaction (important)

Omarchy may run `hyprsunset` (CTM protocol). Ember uses
`wlr-gamma-control` (gamma LUT). They are different hardware stages and can
both be active; the visual effect **compounds**. Ember:

- reads `hyprctl` nightlight state;
- shows a note in the panel/diagnostics when nightlight is on;
- offers an explicit **Turn Off Nightlight** action;
- never turns it off automatically, even when Ember itself is toggled.

Recommended: pick one mechanism for general use. If you prefer Ember's
per-display baselines, disable/hide the default `hyprsunset` autostart; if you
prefer Omarchy's nightlight, leave Ember's Sun schedule off.

## Hardware acceptance checklist — Intel MacBook Pro running Omarchy

Run these on the actual machine, in a graphical session. Record `uname -a`,
`omarchy-version`, `hyprctl version`, `echo $XDG_SESSION_TYPE`, and the output of
`emberctl probe --json`.

### 1. Environment

- [ ] `echo $XDG_SESSION_TYPE` → `wayland`
- [ ] `hyprctl monitors -j` shows the internal panel (usually `eDP-1`) and the
      expected resolution/scale
- [ ] `ls /sys/class/backlight` shows `intel_backlight`, `apple_backlight`, or a
      single panel device
- [ ] `emberctl probe --json` reports backend `wayland`, protocol available and
      one built-in display (no writes); inspect `/sys/class/backlight` separately
- [ ] `ember --version` → `0.4.0 (build 5, Omarchy) · GTK 4`

### 2. Install

- [ ] `makepkg -si` succeeds; `pacman -Ql project-ember` lists the binaries,
      desktop file, icons, unit, D-Bus file
- [ ] `systemctl --user status project-ember` is active; no permission errors
- [ ] The tray icon appears in the Omarchy bar with a rendered flame
- [ ] Left-click opens the layer-shell panel under the bar (not a tiled window)
- [ ] Panel is 390 px wide, fully visible, not clipped at 100 %/125 %/150 %
      scale
- [ ] `journalctl --user -u project-ember -b` shows no panics or repeated
      errors

### 3. Gamma behavior

- [ ] `emberctl status` reports `wayland` and the internal display with
      `Accepted by compositor` (no false readback)
- [ ] Toggle on: screen becomes warmer; slider at 62 % / 75 %
- [ ] Move warmth 0 → 100: smooth, no compounding when moving back and forth
- [ ] Presets Neutral / Evening / Pure Red visibly different; Pure Red has only
      the red channel
- [ ] Software brightness 10 % is dim but usable; 100 % returns to baseline
- [ ] Toggle off: colors return to the exact pre-Ember appearance
- [ ] Quit: `emberctl quit` restores; relaunch shows off (or the preserved
      preference)

### 4. Restore and crash safety

- [ ] With Ember on (Evening), `kill -9 $(pidof emberd)`: Hyprland restores its
      LUT on socket close; the screen returns to normal (backlight may remain
      locked)
- [ ] Relaunch `ember`: startup recovery restores Backlight Lock and starts off
- [ ] Delete/corrupt the journal while stopped:
      `echo garbage > ~/.local/state/project-ember/display-recovery-v1.json`;
      start Ember → attention "journal could not be read", screen untouched,
      `.corrupt-*` file exists
- [ ] `emberctl restore` recovers from a deliberately wrong tint
- [ ] Sleep/wake: colors restore before suspend; wake re-evaluates and stays
      consistent

### 5. Backlight Lock

- [ ] Enabling Backlight Lock reports locked (100 %) and the panel gets
      brighter while Ember dims in software
- [ ] `cat /sys/class/backlight/*/brightness` is at max; `actual_brightness`
      agrees
- [ ] Disabling restores the previous raw value (compare before/after)
- [ ] Quit restores the previous value
- [ ] After a forced crash, restart restores the journaled value
- [ ] On an external monitor, Backlight Lock stays off/unavailable; Ember does
      not touch DDC/CI
- [ ] If the kernel rejects writes, the UI says unavailable/failed, not
      "locked"

### 6. Multi-display / hot-plug

- [ ] Connect an external display: it appears with its own connector name and
      verified state; internal display keeps its tint
- [ ] Unplug it while Ember is on: it becomes **pending**, internal display is
      untouched
- [ ] Replug the **same** display: its saved baseline is restored and it returns
      to the intended warmth
- [ ] Plug a different monitor into the same port: Ember does not inherit the
      old display's baseline (may require EDID difference)
- [ ] Mirroring: Ember does not apply two conflicting transforms; only one
      identity is driven
- [ ] X11 session (if tested): readback verification says `Verified by readback`;
      no XWayland fallback under Hyprland

### 7. Sun schedule

- [ ] Enable Sun schedule without location → row asks for location; manual
      dialog accepts e.g. `52.5 / 13.4`
- [ ] With location: next sunrise/sunset shown in local time; schedule triggers
      at the boundary
- [ ] Change timezone/clock: schedule recalculates, no stuck state
- [ ] `emberctl schedule on` also enables Launch at login; `emberctl schedule
      off` disables autostart (if the user hasn't independently enabled it)
- [ ] Polar case: use a far-north coordinate in winter; no bogus sunrise event

### 8. Omarchy integration

- [ ] Omarchy menu shows Ember rows; each launches the expected action
- [ ] Keybindings work after `hyprctl reload`
- [ ] Tray right-click menu: Open Controls, Toggle, Presets, Sun schedule,
      Diagnostics, Restore, Quit
- [ ] `hyprsunset` active: panel shows the note; **Turn Off Nightlight** works
      and does not crash
- [ ] Uninstall (`pacman -R project-ember` or `scripts/uninstall.sh`) leaves no
      stale tray icon; user data remains unless purged

### 9. Portability pass (second machine)

Repeat on any other Omarchy PC (x86_64 or aarch64, single monitor) and, if
available, a plain X11 session:
- [ ] install, tray, on/off, warmth, brightness, restore
- [ ] no GPU-specific crashes/Banding; note driver and GPU
- [ ] `emberctl simulate all` still passes

## If something goes wrong

```sh
emberctl status
emberctl diagnostics --export ~/ember-diagnostics.txt
journalctl --user -u project-ember -b --no-pager
emberctl restore
```

Collect the diagnostics export and the journal; they contain salted display
tags and no serial numbers. Open an issue with the Omarchy version, Hyprland
version, GPU/driver, `emberctl probe --json`, and reproduction steps.

## Known Omarchy-specific notes

- Sway/wlroots also support the protocol; the panel falls back to a normal
  window because layer-shell may not be available.
- Omarchy's default bar is Quickshell; SNI registration requires the
  `StatusNotifierWatcher`. If the bar is restarted, the tray re-registers.
- `hyprctl` must be on `PATH`; without it, nightlight/mirroring notes are
  omitted but gamma still works.
- The panel itself is affected by the gamma LUT (the whole compositor output
  is); this is expected and matches the macOS behavior.
- On mixed-DPI multi-monitor setups, GTK scale is per-monitor; the 390 px panel
  may need the Omarchy scale setting at 1.0 for pixel-exact screenshots.
