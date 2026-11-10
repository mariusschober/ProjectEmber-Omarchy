#!/usr/bin/env bash
# Pin the Project Ember StatusNotifierItem to Omarchy's bar.
#
# Omarchy puts new tray items in the tray drawer by default. Ember should live
# directly on the bar, like the macOS menu-bar item, so --omarchy runs this.
set -euo pipefail

ID="project-ember"
WIDGET="omarchy.tray"

if ! command -v omarchy >/dev/null 2>&1; then
  echo "omarchy CLI not found; pin Ember from the tray chevron menu instead." >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq not found; pin Ember from the tray chevron menu instead." >&2
  exit 1
fi

current=$(omarchy bar get "$WIDGET" pinned 2>/dev/null || printf '[]')
[[ -n "$current" ]] || current='[]'
updated=$(jq -cn --argjson current "$current" --arg id "$ID" \
  '((if ($current|type) == "array" then $current else [] end) + [$id]) | unique') || exit 1

if [[ "$updated" == "$current" ]]; then
  echo "Project Ember is already pinned in the Omarchy bar."
  exit 0
fi

if omarchy bar set "$WIDGET" pinned "$updated" --json >/dev/null 2>&1; then
  echo "Pinned Project Ember in the Omarchy bar."
else
  echo "Could not pin automatically; right-click the tray chevron and pin Ember." >&2
  exit 1
fi
