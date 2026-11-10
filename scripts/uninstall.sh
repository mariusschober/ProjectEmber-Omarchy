#!/usr/bin/env bash
# Remove Project Ember files. User data is left in place unless --purge.
set -euo pipefail

PREFIX="/usr/local"
USER_INSTALL=0
PURGE=0
DRY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) USER_INSTALL=1; PREFIX="$HOME/.local"; shift ;;
    --prefix) PREFIX="${2:?}"; shift 2 ;;
    --purge) PURGE=1; shift ;;
    --dry-run) DRY=1; shift ;;
    --help|-h) echo "Usage: $0 [--user] [--prefix PATH] [--purge] [--dry-run]"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

if [[ "$USER_INSTALL" == "1" ]]; then
  SYSTEMD_DIR="$HOME/.config/systemd/user"
  DBUS_DIR="$HOME/.local/share/dbus-1/services"
  DESKTOP_DIR="$HOME/.local/share/applications"
  ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
  METADATA_DIR="$HOME/.local/share/metainfo"
  DOC_DIR="$HOME/.local/share/doc/project-ember"
else
  SYSTEMD_DIR="$PREFIX/lib/systemd/user"
  DBUS_DIR="$PREFIX/share/dbus-1/services"
  DESKTOP_DIR="$PREFIX/share/applications"
  ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
  METADATA_DIR="$PREFIX/share/metainfo"
  DOC_DIR="$PREFIX/share/doc/project-ember"
fi
TARGET_HOME="$HOME"
if [[ "$USER_INSTALL" == "0" && "$(id -u)" == "0" && -n "${SUDO_USER:-}" ]]; then
  target_home=$(getent passwd "$SUDO_USER" | cut -d: -f6)
  [[ -n "$target_home" ]] && TARGET_HOME="$target_home" && unset XDG_CONFIG_HOME
fi
AUTOSTART="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/autostart/project-ember.desktop"
MENU="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/omarchy/extensions/project-ember.jsonc"
# Never remove the user's shared omarchy-menu.jsonc.

remove() {
  if [[ -e "$1" ]]; then
    if [[ "$DRY" == "1" ]]; then echo "[dry] rm $1"; else rm -f "$1"; fi
  fi
}

# Ask the running daemon to restore displays before removal.
if command -v emberctl >/dev/null 2>&1; then
  emberctl restore >/dev/null 2>&1 || true
  emberctl quit >/dev/null 2>&1 || true
fi

remove "$PREFIX/bin/emberd"
remove "$PREFIX/bin/emberctl"
remove "$PREFIX/bin/ember"
remove "$SYSTEMD_DIR/project-ember.service"
remove "$DBUS_DIR/org.mariusschober.ProjectEmber.service"
remove "$DESKTOP_DIR/project-ember.desktop"
remove "$ICON_DIR/project-ember.svg"
remove "$ICON_DIR/project-ember-symbolic.svg"
remove "$METADATA_DIR/org.mariusschober.ProjectEmber.metainfo.xml"
remove "$AUTOSTART"
remove "$MENU"
if [[ "$PURGE" == "1" ]]; then
  CONFIG_DIR="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/project-ember"
  STATE_DIR="${XDG_STATE_HOME:-$TARGET_HOME/.local/state}/project-ember"
  remove "$CONFIG_DIR/settings.json"
  remove "$STATE_DIR/display-recovery-v1.json"
  remove "$STATE_DIR/display-recovery-v1.last-good.json"
  remove "$STATE_DIR/diagnostics.log"
  remove "$STATE_DIR/diagnostics.salt"
fi
rm -rf "$DOC_DIR" 2>/dev/null || true
echo "Project Ember removed."
