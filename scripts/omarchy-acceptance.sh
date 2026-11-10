#!/usr/bin/env bash
# Acceptance probe for a real Omarchy machine. It reads display state and may
# start the daemon (read-only; it does not toggle the filter).
set -euo pipefail
say() { printf '%-34s %s\n' "$1" "$2"; }
ok() { say "$1" "OK"; }
fail() { say "$1" "FAIL"; failures=$((failures+1)); }
skip() { say "$1" "SKIP ($2)"; }
failures=0

say "session" "${XDG_SESSION_TYPE:-unknown}"
say "desktop" "${XDG_CURRENT_DESKTOP:-unknown}"
say "kernel" "$(uname -sr)"
say "arch" "$(uname -m)"

if command -v omarchy-version >/dev/null 2>&1; then
  ok "omarchy-version: $(omarchy-version 2>/dev/null | head -1)"
else
  skip "omarchy-version" "command not found"
fi

if [[ "${XDG_SESSION_TYPE:-}" == "wayland" ]]; then ok "Wayland session"; else fail "Wayland session expected"; fi

if command -v hyprctl >/dev/null 2>&1; then
  if hyprctl monitors -j >/dev/null 2>&1; then ok "hyprctl monitors"; else fail "hyprctl monitors"; fi
  ok "hyprland: $(hyprctl version 2>/dev/null | head -1)"
else
  skip "hyprctl" "not installed"
fi

if command -v emberctl >/dev/null 2>&1; then
  ok "emberctl: $(emberctl version 2>/dev/null | head -1)"
  if emberctl probe --json >/tmp/ember-probe.$$ 2>/dev/null; then
    ok "probe: $(head -c 240 /tmp/ember-probe.$$)"
    rm -f /tmp/ember-probe.$$
  else
    fail "emberctl probe"
  fi
  if emberctl status >/dev/null 2>&1; then ok "daemon status"; else fail "daemon status"; fi
else
  fail "emberctl not installed"
fi

if command -v ember >/dev/null 2>&1; then ok "ember: $(ember --version 2>/dev/null)"; else fail "ember not installed"; fi

backlights=$(ls /sys/class/backlight 2>/dev/null | tr '\n' ' ')
if [[ -n "$backlights" ]]; then ok "backlight devices: $backlights"; else fail "no backlight device"; fi

if command -v brightnessctl >/dev/null 2>&1; then ok "brightnessctl present"; else skip "brightnessctl" "optional"; fi
if ldconfig -p 2>/dev/null | grep -q "libgtk4-layer-shell"; then ok "gtk4-layer-shell library present"; else skip "gtk4-layer-shell" "panel falls back to a window"; fi
if [[ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/bus" ]]; then ok "session bus socket"; else fail "session bus socket missing"; fi

echo
if [[ "$failures" -eq 0 ]]; then
  echo "Acceptance probe: no failures. Continue with the interactive checklist in docs/OMARCHY.md."
  exit 0
else
  echo "Acceptance probe: $failures failure(s). See docs/OMARCHY.md."
  exit 1
fi
