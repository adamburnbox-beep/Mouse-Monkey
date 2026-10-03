#!/usr/bin/env bash
# Adds Mouse Monkey to your app menu and desktop, so you can start and stop him
# with a double-click instead of a terminal command. Run it once from the project folder:
#
#   ./install.sh               install
#   ./install.sh --autostart   install, and also start him when you log in
#   ./install.sh --uninstall   remove everything this script added
#
# If you move the project folder, run it again.

set -eu

PROJECT_DIR="$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"
LAUNCHER="$PROJECT_DIR/mouse-monkey.sh"
APPS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
BIN_LINK="$HOME/.local/bin/mouse-monkey"
DESKTOP_DIR="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
ENTRY_NAME="mouse-monkey.desktop"

# Quotes a path for a .desktop Exec line: wrap it in double quotes with " ` $ \
# backslash-escaped, then double every backslash for the file's string escaping.
desktop_exec_quote() {
    printf '"%s"' "$(printf '%s' "$1" | sed -e 's/[\\"`$]/\\&/g' -e 's/\\/\\\\/g')"
}

uninstall() {
    rm -f "$APPS_DIR/$ENTRY_NAME" "$AUTOSTART_DIR/$ENTRY_NAME" "$DESKTOP_DIR/$ENTRY_NAME"
    if [ -L "$BIN_LINK" ]; then rm -f "$BIN_LINK"; fi
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS_DIR" 2>/dev/null || true
    echo "Removed Mouse Monkey from the app menu, desktop and autostart."
}

# Prints the .desktop entry. An optional argument is passed to the launcher on the main
# Exec line (the autostart entry uses "start" so logging in never stops him).
write_entry() {
    local exec_cmd icon
    exec_cmd="$(desktop_exec_quote "$LAUNCHER")"
    icon="$(printf '%s' "$PROJECT_DIR/assets/icon.png" | sed 's/\\/\\\\/g')"
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Mouse Monkey
GenericName=Desktop Pet
Comment=Click to bring the monkey out. Click again to send him home.
Exec=$exec_cmd${1:+ $1}
Icon=$icon
Terminal=false
StartupNotify=false
Categories=Utility;Amusement;
Keywords=monkey;pet;companion;
Actions=Quit;

[Desktop Action Quit]
Name=Send Him Home (Quit)
Exec=$exec_cmd stop
EOF
}

case "${1:-}" in
    --uninstall) uninstall; exit 0 ;;
    --autostart|"") ;;
    *) echo "Usage: $0 [--autostart|--uninstall]" >&2; exit 2 ;;
esac

chmod +x "$LAUNCHER"

# Build now, so the first double-click starts him straight away.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if command -v cargo >/dev/null 2>&1; then
    echo "Building Mouse Monkey (the first time takes a minute or two)..."
    (cd "$PROJECT_DIR" && cargo build --release)
else
    echo "Warning: Rust isn't installed (https://rustup.rs). He'll be built the first time you start him." >&2
fi

# App menu entry (search for "Mouse Monkey", or pin it to your dock/panel).
mkdir -p "$APPS_DIR"
write_entry >"$APPS_DIR/$ENTRY_NAME"
chmod +x "$APPS_DIR/$ENTRY_NAME"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS_DIR" 2>/dev/null || true
echo "Added Mouse Monkey to your app menu."

# Desktop icon, if you have a desktop folder.
if [ -d "$DESKTOP_DIR" ]; then
    write_entry >"$DESKTOP_DIR/$ENTRY_NAME"
    chmod +x "$DESKTOP_DIR/$ENTRY_NAME"
    # Some file managers only run desktop launchers that are marked as trusted.
    command -v gio >/dev/null 2>&1 && gio set "$DESKTOP_DIR/$ENTRY_NAME" metadata::trusted true 2>/dev/null || true
    echo "Added a Mouse Monkey icon to $DESKTOP_DIR."
fi

# A short terminal command too, if you ever want one: mouse-monkey
mkdir -p "$(dirname "$BIN_LINK")"
ln -sfn "$LAUNCHER" "$BIN_LINK"

if [ "${1:-}" = "--autostart" ]; then
    mkdir -p "$AUTOSTART_DIR"
    write_entry start >"$AUTOSTART_DIR/$ENTRY_NAME"
    echo "He'll also start automatically when you log in."
fi

echo
echo "Done! Open Mouse Monkey from your app menu (or double-click the desktop icon)."
echo "Open it again to send him home, or right-click it and choose \"Send Him Home (Quit)\"."
