#!/usr/bin/env bash
# Project Ember installer for Omarchy / Arch Linux.
#
#   ./scripts/install.sh                 # /usr/local, autostart, no config edits
#   ./scripts/install.sh --user          # ~/.local, no root
#   ./scripts/install.sh --omarchy       # also add Omarchy menu entries
#   ./scripts/install.sh --no-autostart  # install files only
#   ./scripts/install.sh --dry-run
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
PREFIX="/usr/local"
USER_INSTALL=0
AUTOSTART=1
BUILD=1
OMARCHY=0
DRY_RUN=0

usage() {
  cat <<'USAGE'
Project Ember installer

  --user              Install into ~/.local and ~/.config (no sudo)
  --prefix PATH       Install prefix (default /usr/local)
  --no-autostart      Do not add the XDG autostart entry
  --no-build          Use an existing target/release build
  --omarchy           Install the Omarchy menu extension
  --dry-run           Print what would happen
  --help              Show this help

After installation:
  systemctl --user daemon-reload        (optional; autostart works without it)
  ember                                 open the control panel
  emberctl status                       check state from a terminal
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) USER_INSTALL=1; PREFIX="$HOME/.local"; shift ;;
    --prefix) PREFIX="${2:?--prefix needs a path}"; shift 2 ;;
    --no-autostart) AUTOSTART=0; shift ;;
    --no-build) BUILD=0; shift ;;
    --omarchy) OMARCHY=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

# When run through sudo, user-facing files belong to the invoking user, not root.
TARGET_HOME="$HOME"
TARGET_UID=""
if [[ "$USER_INSTALL" == "0" && "$(id -u)" == "0" && -n "${SUDO_USER:-}" ]]; then
  target_home=$(getent passwd "$SUDO_USER" | cut -d: -f6)
  if [[ -n "$target_home" ]]; then
    TARGET_HOME="$target_home"
    TARGET_UID=$(id -u "$SUDO_USER")
    unset XDG_CONFIG_HOME
  fi
fi

if [[ "$USER_INSTALL" == "1" ]]; then
  BIN_DIR="$PREFIX/bin"
  SYSTEMD_DIR="$HOME/.config/systemd/user"
  DBUS_DIR="$HOME/.local/share/dbus-1/services"
  DESKTOP_DIR="$HOME/.local/share/applications"
  ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
  METADATA_DIR="$HOME/.local/share/metainfo"
  DOC_DIR="$HOME/.local/share/doc/project-ember"
  AUTOSTART_DIR="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/autostart"
else
  BIN_DIR="$PREFIX/bin"
  SYSTEMD_DIR="$PREFIX/lib/systemd/user"
  DBUS_DIR="$PREFIX/share/dbus-1/services"
  DESKTOP_DIR="$PREFIX/share/applications"
  ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
  METADATA_DIR="$PREFIX/share/metainfo"
  DOC_DIR="$PREFIX/share/doc/project-ember"
  AUTOSTART_DIR="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/autostart"
fi

step() { printf '  %s\n' "$*"; }
run() {
  if [[ "$DRY_RUN" == "1" ]]; then
    printf '  [dry] %s\n' "$*"
  else
    "$@"
  fi
}
install_file() {
  local mode="$1" src="$2" dst="$3"
  run install -Dm"$mode" "$src" "$dst"
}

if [[ "$BUILD" == "1" ]]; then
  echo "Building Project Ember (release)…"
  (cd "$ROOT" && cargo build --release --locked)
fi

for binary in emberd emberctl ember; do
  if [[ ! -x "$TARGET_DIR/release/$binary" ]]; then
    echo "Missing $TARGET_DIR/release/$binary; run without --no-build." >&2
    exit 1
  fi
done

echo "Installing Project Ember"
step "prefix: $PREFIX"
install_file 755 "$TARGET_DIR/release/emberd" "$BIN_DIR/emberd"
install_file 755 "$TARGET_DIR/release/emberctl" "$BIN_DIR/emberctl"
install_file 755 "$TARGET_DIR/release/ember" "$BIN_DIR/ember"

# Desktop entry with an absolute Exec path so custom prefixes work.
tmp_desktop=$(mktemp)
sed "s|^Exec=ember$|Exec=$BIN_DIR/ember|" "$ROOT/packaging/project-ember.desktop" > "$tmp_desktop"
install_file 644 "$tmp_desktop" "$DESKTOP_DIR/project-ember.desktop"
rm -f "$tmp_desktop"

# Systemd user unit with the configured binary path.
tmp_service=$(mktemp)
sed "s|^ExecStart=/usr/bin/emberd$|ExecStart=$BIN_DIR/emberd|" \
  "$ROOT/packaging/project-ember.service" > "$tmp_service"
install_file 644 "$tmp_service" "$SYSTEMD_DIR/project-ember.service"
rm -f "$tmp_service"

# Session-bus activation, used by emberctl and the GUI to start the daemon.
tmp_dbus=$(mktemp)
sed "s|^Exec=/usr/bin/emberd$|Exec=$BIN_DIR/emberd|" \
  "$ROOT/packaging/org.mariusschober.ProjectEmber.service" > "$tmp_dbus"
install_file 644 "$tmp_dbus" "$DBUS_DIR/org.mariusschober.ProjectEmber.service"
rm -f "$tmp_dbus"

install_file 644 "$ROOT/packaging/icons/project-ember.svg" "$ICON_DIR/project-ember.svg"
install_file 644 "$ROOT/packaging/icons/project-ember-symbolic.svg" "$ICON_DIR/project-ember-symbolic.svg"
install_file 644 "$ROOT/packaging/org.mariusschober.ProjectEmber.metainfo.xml" \
  "$METADATA_DIR/org.mariusschober.ProjectEmber.metainfo.xml"
install_file 644 "$ROOT/LICENSE" "$DOC_DIR/LICENSE"
install_file 644 "$ROOT/README.md" "$DOC_DIR/README.md"
install_file 644 "$ROOT/docs/SYSTEM_DESIGN.md" "$DOC_DIR/SYSTEM_DESIGN.md"
install_file 644 "$ROOT/docs/SAFETY.md" "$DOC_DIR/SAFETY.md" 2>/dev/null || true

if [[ "$AUTOSTART" == "1" ]]; then
  tmp_autostart=$(mktemp)
  sed "s|^Exec=emberd$|Exec=$BIN_DIR/emberd|" \
    "$ROOT/packaging/project-ember-autostart.desktop" > "$tmp_autostart"
  install_file 644 "$tmp_autostart" "$AUTOSTART_DIR/project-ember.desktop"
  rm -f "$tmp_autostart"
  step "autostart: $AUTOSTART_DIR/project-ember.desktop"
else
  step "autostart: skipped"
fi

if [[ "$OMARCHY" == "1" ]]; then
  EXT_DIR="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/omarchy/extensions"
  MENU="$EXT_DIR/omarchy-menu.jsonc"
  # Omarchy reads exactly this file. Never overwrite a user's menu: install
  # the snippet when the file does not exist, otherwise keep it beside the
  # docs and print the merge instruction.
  if [[ ! -e "$MENU" ]]; then
    install_file 644 "$ROOT/packaging/omarchy/omarchy-menu.jsonc" "$MENU"
    step "Omarchy menu extension: $MENU"
  else
    step "Omarchy menu file exists; not modified."
    step "Add the entries from $DOC_DIR/omarchy-menu.jsonc to $MENU"
  fi
  install_file 644 "$ROOT/packaging/omarchy/omarchy-menu.jsonc" \
    "$DOC_DIR/omarchy-menu.jsonc"
  install_file 644 "$ROOT/packaging/omarchy/hypr-bindings.lua" \
    "$DOC_DIR/hypr-bindings.lua"
  step "Optional keybindings: $DOC_DIR/hypr-bindings.lua"

  # New SNI items land in Omarchy's tray drawer. Pin Ember to the bar so it is
  # visible immediately, like the macOS menu-bar icon.
  if [[ -x "$ROOT/scripts/omarchy-pin-tray.sh" ]]; then
    if "$ROOT/scripts/omarchy-pin-tray.sh"; then
      step "Tray: pinned to the Omarchy bar"
    else
      step "Tray: pin Ember from the tray chevron menu (right-click)."
    fi
  fi
fi

if [[ "$DRY_RUN" == "0" && -n "$TARGET_UID" ]]; then
  [[ -e "$AUTOSTART_DIR/project-ember.desktop" ]] && chown "$TARGET_UID" "$AUTOSTART_DIR/project-ember.desktop" || true
  [[ -e "${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/omarchy/extensions/omarchy-menu.jsonc" ]] && \
    chown "$TARGET_UID" "${XDG_CONFIG_HOME:-$TARGET_HOME/.config}/omarchy/extensions/omarchy-menu.jsonc" || true
fi

if [[ "$DRY_RUN" == "0" ]]; then
  if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$DESKTOP_DIR/project-ember.desktop" || true
  fi
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q "$(dirname "$(dirname "$ICON_DIR")")" >/dev/null 2>&1 || true
  fi
fi

cat <<NEXT

Installed.
  binaries:  $BIN_DIR/{emberd,emberctl,ember}
  daemon:    $SYSTEMD_DIR/project-ember.service
  D-Bus:     $DBUS_DIR/org.mariusschober.ProjectEmber.service

Next:
  ember                       open the control panel
  emberctl status             check without opening the GUI
  systemctl --user daemon-reload
  systemctl --user start project-ember   (optional; autostart also works)

Omarchy: bind "ember" to a key in ~/.config/hypr/bindings.lua, e.g.
  o.bind("SUPER + SHIFT + E", "Ember controls", "ember")

If Ember was already running, restart it so the new binaries and tray code load:
  emberctl quit && ember
NEXT
