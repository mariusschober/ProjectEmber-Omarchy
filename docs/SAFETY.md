# Safety model

Project Ember changes the display transfer table (and, for Backlight Lock, the
panel backlight). A bug can leave a screen dim, tinted, or at full hardware
brightness. The design therefore treats display state as recoverable user data,
not as a cache.

## Non-negotiable rules

1. **Journal before mutation.** No gamma or backlight write happens until the
   per-display baseline and intended settings are durably on disk (atomic
   replace, fsync, 0600).
2. **Verified restore before deletion.** A journal entry is never removed just
   because a restore was *attempted*. It is removed only after the backend
   confirms restoration (readback on X11, accepted/destroyed control on
   Wayland, brightness readback for Backlight Lock).
3. **Immutable baselines.** Every warmth/brightness change is computed from the
   original table, never from the previous modified table. Repeated slider
   movement cannot compound the transform.
4. **No blanket restore.** Hot-plug reconciliation touches only the affected
   identity. Unplugging one monitor must not restore/blank the survivor.
5. **Observed truth drives the UI.** The panel shows "Accepted by compositor" on
   Wayland and "Verified by readback" on X11. It never says "restored" when the
   restore was partial or unverified.
6. **Disconnected means pending.** The entry is retained and restored when that
   exact physical display returns.
7. **Ambiguity means no mutation.** If two online displays cannot be
   distinguished, or a display has no EDID and is not a recognisable built-in
   panel, Ember leaves it alone and says why.
8. **Backlight Lock is reversible and built-in-only.** External DDC/CI, ambient
   light automation and root/KMS writes are out of scope.
9. **No silent fallback.** A failure to use wlr-gamma-control in a Wayland
   session is not silently retried through XWayland; the display is reported
   unsupported.
10. **User preference survives.** Off/quit/sleep preserve the user's desired
    warmth/brightness; crash recovery deliberately starts **off** for safety.

## Recovery journal

Schema 2 JSON:

```json
{
  "schemaVersion": 2,
  "appVersion": "0.4.0 (build 5, Omarchy)",
  "createdAt": 1794168000,
  "displays": [
    {
      "identity": { "uuid": "...", "vendorID": 1552, "productID": 1234,
                    "serial": 1, "unit": 0, "isBuiltIn": true,
                    "connector": "eDP-1", "edidHash": null },
      "baseline": { "gammaSize": 4096, "gammaSource": "readback",
                    "gammaTable": { "blocks": [...] },
                    "hardwareBacklight": 42, "capturedAt": 1794168000 }
    }
  ],
  "intendedSettings": { "warmth": 0.62, "apparentBrightness": 0.75, "...": "..." }
}
```

Guarantees:

- maximum file size 1 MiB; larger files are treated as corrupt without
  allocating/parsing them;
- malformed JSON / wrong types / unknown schema version → **quarantined**
  (`display-recovery-v1.corrupt-<epoch>.json`), never deleted or ignored;
- schema v1 is migrated in memory for recovery (best effort);
- symlinks are refused (`O_NOFOLLOW`), files must be regular, owned
  appropriately and mode 0600; a directory is created 0700;
- writes go to a temporary file in the same directory, fsync, rename, fsync the
  directory; a `.last-good` copy is kept;
- a journal is never cleared when any entry is still pending or any restore
  failed.

## Failure decision tree

| Situation | Behaviour |
|---|---|
| One display unsupported | Others continue; this one reported unsupported; never mutated |
| One display fails to apply | Roll back that display; retain its entry; attention |
| None verify after activation | Attempt rollback; desired state off; attention with Retry |
| Restore fails | Entry retained; runtime Degraded; retry and Reset offered |
| Journal unreadable | Quarantine; no display mutated; attention "journal could not be read" |
| Journal from a newer schema | Treat as unreadable; preserve; attention |
| Daemon crash (SIGKILL) | Compositor resets Wayland LUT on socket close; X11 table remains; backlight may remain locked. Restart performs recovery restore first, then stays off |
| User quits | Restore all; clear journal only on verified success; preserve desired preference |
| Sleep | Restore before suspend; re-evaluate on wake |
| Output identity ambiguous | Skip mutation; UI says so |
| Backlight guard fails 3× | Stop lock, restore saved brightness, attention |
| Nightlight active | Report; never disable automatically; user may click Turn Off |
| External process resets LUT (X11) | Bounded reapply (one per event), 3 in 60 s → degraded, stop fighting |
| Second daemon | Refused by `flock` and by D-Bus name ownership |
| Display unplugged during apply | Command checks generation; failure retained; pending |

## Backlight Lock specifics

Order of operations:

1. Confirm built-in connector and one unambiguous backlight device.
2. Journal the raw hardware brightness **before** writing.
3. Write 100 %, read back, require ≥ 97 %.
4. Every 5 s, verify/repair; three consecutive failures stop the lock.
5. On off/sleep/quit, write back the saved raw value; verify.
6. If verification is impossible, report failure instead of claiming success.

This means a power loss can leave the panel at full hardware brightness until
the daemon restarts; restart recovery restores the journaled value. Ember
cannot prevent that with a userspace daemon, and does not try to write KMS/DDC
behind the user's back.

## Emergency recovery

```sh
emberctl restore       # restore every available display from the journal
emberctl reset         # release controls, clear preferences, restore
emberctl quit          # restore and stop
systemctl --user stop project-ember
rm ~/.local/state/project-ember/display-recovery-v1.json   # last resort, read SAFETY first
```

On Hyprland, quitting the daemon closes the gamma-control socket; the
compositor restores its original LUT even if the journal is lost. X11 does not
have that guarantee; the journal is the protection.

## Threat model boundaries

- The daemon runs as the logged-in user, on the user's session bus. Another
  process with the same UID can call it; that is equivalent to controlling the
  session's display.
- Root, a compromised compositor, or a kernel/GPU driver bug is out of scope.
- Ember does not read the framebuffer, capture windows, or inspect other apps.
- Ember never accepts a display identity or path from an untrusted caller; the
  D-Bus API exposes settings and lifecycle, not arbitrary file/device paths.
