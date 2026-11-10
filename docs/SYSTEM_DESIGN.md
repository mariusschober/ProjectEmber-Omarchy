# System design (Linux / Omarchy 0.4.0)

## Version truth

`ember_core::AppVersion` (marketing 0.4.0, build 5, journal schema 2) is the
single source of truth. Binaries, package metadata, the footer and diagnostics
derive from it. The Linux edition is labelled **Omarchy**.

## Process model

```
ember (GTK4) ──D-Bus──┐
emberctl      ──D-Bus──┤   emberd (one coordinator thread)
tray (ksni)   ──inproc┘        │
                              ├─ DisplayBackend
                              │   ├─ Wayland wlr-gamma-control
                              │   ├─ X11 XRandR
                              │   ├─ simulator
                              │   └─ null (no session)
                              ├─ BacklightControl (logind/brightnessctl/sysfs)
                              ├─ RecoveryJournal + SettingsStore
                              ├─ Solar schedule + GeoClue (background thread)
                              └─ logind sleep watcher
```

- A single `emberd` instance owns all hardware mutation. It is protected by an
  advisory `flock` and by owning the session-bus name
  `org.mariusschober.ProjectEmber`.
- The GUI never mutates displays. It polls a `Snapshot` over D-Bus and sends
  commands. The tray runs inside the daemon.
- The coordinator is a synchronous state machine thread; D-Bus/tokio, GeoClue
  and logind run around it. This makes the safety-critical path deterministic
  and easy to test with fakes.
- Blocking platform calls are short. A hung compositor is bounded by command
  timeouts in the Wayland backend (5 s) and the daemon remains responsive to
  D-Bus reads.

## Safety invariants

1. Journal before mutation.
2. Verified restore before deletion.
3. Immutable original baseline; transforms always derive from it.
4. No blanket restore on topology change.
5. Observed truth drives the UI; a failed/unknown display is never presented as
   active.
6. Disconnected is pending, not forgotten.
7. Ambiguity means no mutation.
8. Backlight Lock is built-in-only and reversible.
9. A stale topology generation cannot win.
10. No silent fallback: capability and verification limits are surfaced.

## Display backends

### Wayland (`zwlr_gamma_control_v1`)

- The daemon binds the manager and one gamma control per `wl_output`.
- Hyprland (verified in its source) and wlroots send `gamma_size` immediately,
  or `failed` if the output has no gamma table, another client holds exclusive
  control, or ICC ramps are in use.
- `set_gamma` takes an fd containing three interleaved `uint16` ramps of
  `gamma_size` entries. The backend writes a `memfd` and round-trips to observe
  protocol errors/`failed`.
- **No readback exists.** Verification is "accepted by compositor"; the UI says
  so. This is recorded in the snapshot as `VerificationLevel::Applied`.
- Destroying the control restores the compositor's original table. That is the
  restore primitive; the saved baseline is identity/compositor-default.
- On daemon crash the socket closes and Hyprland resets the LUT. The journal
  still protects the hardware backlight.
- One controller per output is enforced by Hyprland; a second client gets
  `failed`. Ember surfaces that as an unsupported/attention state rather than
  fighting.

### X11 (`XRandR`)

- Enumerates outputs/CRTCs, reads EDID from the output property, and uses
  `get_crtc_gamma`/`set_crtc_gamma`.
- Every write is read back and compared within 0.004; `VerificationLevel::Readback`.
- A dedicated connection watches RandR events for hot-plug.
- XWayland is never silently used for a Wayland session: its gamma only affects
  X11 clients, not the compositor.

### Simulator / null

The simulator has virtual `eDP-1`/`DP-1`, real gamma math, fault injection
(unplug, reset, failed apply/restore, no-readback mode). It powers
`emberctl simulate`, daemon tests and GUI screenshots. The null backend lets the
daemon/UI start and report "no compatible display" outside a graphical session.

## Identity

Priority: EDID serial → EDID hash → built-in connector → ambiguous.

- Internal connector prefixes: `eDP`, `LVDS`, `DSI`, `DPI`.
- Duplicate online keys are ambiguous and never mutated.
- Journal entries are matched by full identity rules (UUID/serial/vendor/model/
  built-in), never by transient ids. A different monitor on the same port does
  not inherit the old baseline.

## Topology reconciliation

1. A backend event increments `generation`; daemon enters `Reconciling`.
2. Reconcile is debounced 300 ms and bounded at 2 s; two matching snapshots are
   required for "settled" (otherwise a warning is logged).
3. The pure `DisplayReconciler` emits per-identity actions:
   `LeaveVerified`, `ReapplyFromBaseline`, `JournalThenApply`,
   `ApplyPendingBaseline`, `RestoreSavedBaseline`, `RetainPending`,
   `LeaveUnsupported`, `LeaveAmbiguous`.
4. Every delayed action checks the generation before I/O. A stale generation
   aborts and reschedules.
5. Post-event verification at ~0.5 s and ~2 s; health checks every 30 s.
6. On X11, a readback mismatch triggers one bounded reapply. Three resets in
   60 s enter a truthful degraded state with Retry/Reset; Ember pauses
   automatic reapplication instead of fighting another service.
7. On Wayland, there is no readback to detect resets; control-loss events are
   the signal.

## Activation / restore

- Activation enumerates, restores any pending journal entries that returned,
  captures missing baselines, captures a built-in hardware baseline when
  Backlight Lock is on, **saves the journal**, then applies from each immutable
  baseline. A failure on one display never blocks the others.
- If nothing verifies, activation fails, attempts rollback, turns the desired
  filter off and reports attention.
- Restore writes/destroys each display baseline, restores hardware brightness,
  and removes an entry only after success. Offline entries stay pending.
- Sleep restores before suspend and preserves the desired preference; wake
  re-evaluates. Quit/termination restores and preserves the preference; crash
  recovery restores and stays off for safety.

## Backlight Lock

1. Built-in connector + exactly one unambiguous `/sys/class/backlight` device.
2. Capture raw brightness (journaled before write).
3. Write 100 % through logind → `brightnessctl` → sysfs, and verify ≥ 97 %.
4. Guard every 5 s with one bounded write; three failures stop the lock,
   restore the saved brightness, and report a truthful failure.
5. Off/sleep/quit restore hardware brightness; the preference is retained.
6. Ambient-light automation is not controllable; the UI reports sensor presence
   and says so.

## Sun schedule

- Coordinates are rounded to 0.1° and stored locally. GeoClue2 is optional and
  queried from a background thread; denied permission disables the schedule
  with an explanation and a manual-location action.
- The NOAA sunrise/sunset calculation is on-device, DST-aware, polar-aware, and
  searches forward for the next real event.
- Turning the schedule on also enables Launch at login (via XDG autostart).
- Manual on/off while the schedule is enabled becomes an override that expires
  at the next solar boundary.
- Time-zone/clock jumps are detected; the schedule is recalculated.

## Presentation

`EmberPresenter` is a pure function of desired state, runtime state, verified/
unsupported/pending/failed counts, ambiguity and attention. The UI never
reinterprets raw state. `Snapshot` is the single D-Bus JSON and render source.

## Persistence

- Settings: `$XDG_CONFIG_HOME/project-ember/settings.json`, atomic 0600.
- Journal: `$XDG_STATE_HOME/project-ember/display-recovery-v1.json`, schema 2,
  epoch-seconds timestamps, 0600, 1 MiB cap, last-good backup, quarantine.
- Diagnostics: `$XDG_STATE_HOME/project-ember/diagnostics.log`, rotating,
  salted identity tags, control characters stripped.
- Autostart: `$XDG_CONFIG_HOME/autostart/project-ember.desktop`.
- Runtime lock: `$XDG_RUNTIME_DIR/project-ember/daemon.lock`.

## D-Bus API

Bus `org.mariusschober.ProjectEmber`, path
`/org/mariusschober/ProjectEmber`, interface of the same name.

Methods: `GetSnapshot`, `GetVersion`, `SetEnabled`, `Toggle`, `SetWarmth`,
`SetBrightness`, `ApplyPreset`, `SetBacklightLock`, `SetSunSchedule`,
`SetLaunchAtLogin`, `SetTrayPrimaryAction`, `SetLocation`, `ClearLocation`,
`TurnOffNightlight`, `Retry`, `Reset`, `RestoreNow`, `Refresh`, `OpenPanel`,
`OpenDiagnostics`, `ExportDiagnostics`, `Quit`.
Signal: `SnapshotChanged(s snapshotJson)`.

The session bus is per-user; the service is not on the system bus. Inputs are
validated (finite ranges, known presets/actions); invalid calls return
`org.freedesktop.DBus.Error.InvalidArgs`.

## Native Omarchy integration

- `hyprctl` provides read-only monitor mirroring and nightlight state.
- `gtk4-layer-shell` (installed by Omarchy) is loaded at runtime to anchor the
  panel under the bar; absence falls back to a normal window.
- The SNI tray works with Omarchy's Quickshell bar tray.
- An optional JSONC extension adds Ember entries to the Omarchy menu.
- `hyprsunset` uses CTM, not gamma; coexistence is reported, never silently
  modified.
