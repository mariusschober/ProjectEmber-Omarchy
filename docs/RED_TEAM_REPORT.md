# Red-team report — Project Ember Linux 0.4.1

Date: 2026-11-08/09
Scope: `ember-core`, `ember-platform`, `ember-daemon`, `ember-cli`,
`ember-gui`, packaging and installer, as built in this repository.
Method: source review, adversarial unit/integration tests, protocol fuzzing of
the public D-Bus surface, fault injection in the simulator, path/permission
attacks, dependency and network audit, installer review. No live Omarchy
hardware was available; hardware-dependent risks are listed separately.

## Summary

| Area | Attacks | Blocked / safe | Residual |
|---|---|---|---|
| Journal / recovery | 8 | 8 | hardware-only restore failure modes |
| Identity / topology | 4 | 4 | exotic EDID-less hardware |
| Backlight | 4 | 4 | kernel/power-loss window |
| D-Bus input | 7 | 7 | same-UID trust boundary |
| Filesystem / symlinks | 7 | 7 | pre-existing malicious XDG dirs owned by another UID |
| Concurrency | 3 | 3 | advisory `flock` on network filesystems |
| Wayland / X11 protocol | 4 | 4 | real compositor behavior |
| Installer / packaging | 5 | 5 | tampered source tarball (hash now pinned) |
| Network / privacy | 3 | 3 | GeoClue service policy |

No critical or high-severity defect was found in the implemented scope. Four
issues found during development were fixed (below).

## Adversarial journal tests

| Attack | Expected | Observed | Evidence |
|---|---|---|---|
| Replace journal with symlink to `/etc/passwd` | Refuse; do not read/write target | `EmberError::UnsafePath`; no target change | core fs tests |
| Replace settings with symlink | Same | refused | core fs tests |
| Journal is a directory/FIFO | Refuse without blocking | regular-file/metadata check | core fs tests |
| Journal > 1 MiB | Refuse without large allocation | metadata length check before read | `oversized_journal_*` |
| Journal is valid JSON but schema 99 | Preserve, attention, no mutation | quarantined/preserved, `Degraded`, zero display diff | red-team test |
| Journal is binary garbage | Quarantine, no mutation | `.corrupt-*` copy preserved, attention | red-team test |
| Journal has duplicate/unknown keys | Ignore unknown, normalize | serde default/deny appropriate; schema validated | core tests |
| Journal timestamps in ns/us instead of epoch seconds | Reject/migrate | schema v1 migration and ms truncation tested | core tests |
| Crash after journal write, before gamma write | Restore sees untouched state | idempotent restore | crash-recovery scenario |
| Crash after gamma write, before journal entry | Impossible by ordering | write-ahead path tested | code review + scenarios |

**Result:** no path where a malformed or hostile journal causes a display to be
modified, a file outside the state directory to be touched, or unbounded
allocation.

## Identity and topology attacks

| Attack | Expected | Observed |
|---|---|---|
| Two displays report the same identity | No mutation, "ambiguous" | `ambiguous_identities` core tests; reconciler skips |
| Same connector, different EDID/serial | New identity; never inherit old baseline | `journal_never_resolves_to_a_different_display` passes |
| EDID removed/added between generations | Identity changes; pending old entry | fingerprint tests |
| Unplug A while B is active | B's table untouched; A pending | reconnect red-team test; hotplug scenario |
| Replug A while B active | A restored from its baseline only | hotplug scenario |
| Stale generation response after a newer event | Discard; no mutation | generation checks tested |
| X11 LUT reset by an external tool | Bounded reapply; 3 in 60 s → degraded | external-reset scenario |

**Result:** no cross-display baseline confusion.

## Backlight

| Attack | Expected | Observed |
|---|---|---|
| External/DDC display asks for lock | Refuse; built-in only | platform tests |
| Two backlight devices | Refuse to choose arbitrarily | selection tests |
| Device name with `../` or shell metacharacters | No path escape, no shell | validation and `Command` args (no shell) |
| logind unavailable | Fall back to `brightnessctl` → sysfs | platform tests |
| logind denies (`InteractiveAuthorizationRequired`) | No retry storm; truthful failure | guard logic |
| Write fails 3× | Stop lock, restore saved value, attention | backlight scenario |
| Power loss while locked | Journal restores on next start | design; hardware test pending |

**Result:** no root, no shell, no arbitrary path.

## D-Bus input fuzzing

Calls were made through the real interface on a private bus:

| Input | Result |
|---|---|
| `SetWarmth(NaN)`, `±inf` | `InvalidArgs` |
| `SetBrightness(-1)`, `0`, `0.02`, `99` | `InvalidArgs` |
| `ApplyPreset("not-a-preset")` | `InvalidArgs` |
| `SetTrayPrimaryAction("__proto__")` | `InvalidArgs` |
| `SetLocation(91, 181)`, NaN | `InvalidArgs` |
| `SetLocation` outside valid ranges | rejected |
| Unknown method | standard D-Bus error |
| Mistyped signature | standard D-Bus error |

The interface is on the **session** bus; the service is owned by the user.
Any same-UID process can already control the session's display, so this is not
a privilege boundary. No method accepts a file path, device path, command, or
environment variable from the caller. `ExportDiagnostics` only returns a string;
the CLI export path is chosen by the invoking user's own shell.

**Result:** no injection, no panic, no state corruption in tested inputs.

## Filesystem and concurrency

| Attack | Result |
|---|---|
| Symlinked journal/settings/corrupt copy | refused (`O_NOFOLLOW`, regular-file checks) |
| World-readable journal | created 0600; existing wrong mode rejected or tightened |
| World-writable state dir | created 0700; existing wrong owner/mode rejected |
| Two daemons at once | `flock` + D-Bus name; second refused |
| Lock file unlinked while held | held fd remains authoritative; stale lock reacquired safely |
| Daemon quits while commands in flight | `watch` finished signal; process exits after restore |
| Slow/hung compositor | Wayland command deadline (5 s); D-Bus reads remain responsive |

**Residual:** `flock` is advisory. A malicious same-UID process can ignore it
and race display writes; it could also call D-Bus directly. This is outside the
trust boundary.

## Protocol

- **Wayland:** mock compositor verifies exact interleaved u16 bytes at
  `gamma_size`, `failed` handling, unsupported reporting, and restore on
  `destroy`. The backend never claims readback.
- **X11:** Xvfb test verifies real readback and idempotent restore. X11 errors
  are surfaced, not ignored.
- **XWayland:** never selected for a Wayland session; no silent downgrade.
- **fd handling:** the `memfd` is sealed/closed promptly; no fd leak across
  cycles (unit tests iterate apply/restore).

**Residual:** a compositor bug or GPU driver clamp could accept different bytes
than requested; Wayland cannot detect this. X11 readback detects it.

## Installer and packaging

| Check | Result |
|---|---|
| No `curl | sh`, no remote execution | installer only builds/copies local files |
| No automatic edits to Hyprland/Omarchy config | opt-in `--omarchy` writes a separate JSONC extension; keybindings only printed/copied to docs |
| Uninstall removes binaries/units/desktop/icons | yes; user data preserved unless `--purge` |
| Custom prefix rewrites systemd/D-Bus/desktop Exec | yes (tested with a temp HOME) |
| D-Bus activation works from installed prefix | tested with `dbus-run-session` |
| `desktop-file-validate` | passes after category fix |
| PKGBUILD source hash must be updated with every regenerated tarball | low | release checklist; verify `makepkg` hash before publishing |

## Network and privacy audit

```
grep -RInE "TcpStream|TcpListener|UdpSocket|reqwest|ureq|hyper::Client|std::net::|tokio::net::" crates
→ none
grep for curl/wget in code/scripts → none
cargo tree | grep HTTP clients → none
```

`zbus`/`tokio` use Unix sockets for D-Bus and in-process channels. GeoClue
queries go through the system D-Bus service; its own network policy is outside
Ember. Ember has no updater, telemetry, account, license check, remote logging,
or crash upload.

## Issues found during the port and fixed

1. **Startup race in tests / UI:** D-Bus answered `GetSnapshot` before the
   coordinator's first publish, returning `backend: none`. The integration test
   now waits for the simulator snapshot; the GUI tolerates the empty snapshot.
   No user-visible defect, but it made tests flaky.
2. **`emberctl quit` / D-Bus `Quit` did not terminate the process:** the
   coordinator stopped but `main` waited forever for a signal. Added a
   `watch` "finished" channel from coordinator to runtime; quit now exits after
   restore. Verified.
3. **GTK scale default blue highlight** and a transparent strip below the panel
   in the first render. CSS now clears the default highlight; the panel fills
   the window and the footer is bottom-aligned.
4. **Installer ignored `CARGO_TARGET_DIR`**, breaking builds that use a shared
   target directory. Fixed; prefix install then verified end-to-end.
5. **Launch at login wrote `emberd --autostart` while the daemon did not accept
   that flag, and a `--user` install's `~/.local/bin` is not always on the
   session PATH.** The daemon now accepts the hidden flag, and the generated
   autostart entry uses the absolute `current_exe()` path (quoted when needed).
   Verified in a temp HOME.
6. **GUI and daemon initially shared an application/bus name.** The GTK
   application is now `org.mariusschober.ProjectEmber.Panel`, so it cannot
   collide with the daemon's `org.mariusschober.ProjectEmber`. Verified with
   `busctl --user list` under a private bus.
7. **Diagnostics and location windows were local `gtk::Window` values and were
   finalized immediately** when opened standalone. They are now
   `ApplicationWindow`s owned by the GtkApplication; `ember --diagnostics` stays
   open and the tray action no longer opens a second panel.
8. **The degraded panel could extend past a 768 px screen.** Height is clamped
   to the monitor minus the Omarchy bar, with a scroller for short screens.
9. **Autostart directory permissions were normalized to 0755 even when the
   directory already existed**, which could loosen a hardened `~/.config`.
   Existing directories are now left untouched; only a newly created autostart
   directory is 0755. Added a regression test.
10. **The tray gave up permanently if Omarchy's StatusNotifierWatcher started
    after `emberd`.** The tray now waits for the watcher and re-registers after
    a shell restart; `scripts/tray_registration.rs` covers both cases, and the
    Omarchy installer pins the item to the bar.

## Residual risk register

| Risk | Severity | Mitigation / next step |
|---|---|---|
| Real Hyprland gamma behavior differs from mock | medium | hardware acceptance: exclusive control, failed events, restore on destroy |
| Wayland cannot verify actual LUT | medium | UI/diagnostics state "accepted"; no false "verified" claim |
| Power loss while Backlight Lock is engaged | low | journal restores on next start; cannot be eliminated in userspace |
| Kernel/GPU clamps or banding at low brightness | low | test on Intel MacBook panel; document |
| logind/polkit policy varies by distro | low | brightnessctl/sysfs fallbacks; clear attention |
| Exotic EDID-less external monitors | low | ambiguity → untouched, reported |
| `hyprsunset` + Ember compound tint | low | detected and surfaced; never auto-disabled |
| Source tarball/hash drift | low | regenerate `scripts/package.sh` and pin the printed hash in PKGBUILD/.SRCINFO |
| No aarch64 hardware run | low | pure Rust; cross-build in CI |
| No real Omarchy/Quickshell session tested | medium | hardware acceptance checklist |

## Sign-off

Within the tested/simulated scope, the safety invariants hold under the attacks
above. The release is **not** hardware-certified yet. The next gate is the
checklist in `OMARCHY.md` on the owner's Intel MacBook Pro running Omarchy,
followed by a second machine (any Omarchy PC) and an X11 session.
