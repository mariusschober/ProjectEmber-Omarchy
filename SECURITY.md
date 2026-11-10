# Security policy

## Reporting

Report suspected vulnerabilities privately to the maintainer via
<https://mariusschober.com/>. Do not open a public issue for a safety or
privilege bug before it is triaged. Include the Ember version, session type
(Wayland/X11), compositor, GPU/driver, and a diagnostics export.

## Scope

Project Ember runs as the logged-in user and talks only to the user's session
D-Bus. It does not run as root, does not use the system bus, does not open
`/dev/dri`, does not read the framebuffer, and makes no network requests.

In scope:

- display/backlight corruption or failure to restore;
- path traversal, symlink attacks, unsafe file permissions;
- command injection in backlight/hyprctl/autostart handling;
- denial of service via D-Bus (unbounded memory, panics, deadlocks);
- journal corruption leading to wrong-display restoration;
- installer writing outside the requested prefix;
- privacy leaks in diagnostics.

Out of scope:

- a same-UID process calling the session D-Bus API (it can already control the
  session display);
- root, kernel, GPU driver or compositor compromise;
- physical access, power loss, or hardware failure;
- GeoClue's own network behavior (Ember only calls the system service).

## Design guarantees

See `docs/SAFETY.md`. In short: journal before mutation, verified restore before
journal deletion, immutable baselines, no blanket restore, observed truth in the
UI, disconnected = pending, ambiguity = no mutation, built-in-only reversible
backlight, no silent fallback.

## Permissions

- Settings/journal/diagnostics: `0600`; directories `0700`.
- Symlinks refused; atomic temp-file + rename writes.
- No setuid/setgid binaries, no capabilities, no polkit policy installed.
- systemd user unit has `NoNewPrivileges=true` and `PrivateTmp=true`.

## Dependencies

Rust dependencies are pinned by `Cargo.lock`. The main trusted components are
`gtk4`, `wayland-client`, `x11rb`, `zbus`, `ksni`, `chrono`, `serde`. Run
`cargo audit` before a release and keep `Cargo.lock` updated deliberately.

## License

Project Ember is released under the MIT license. See `LICENSE`.
