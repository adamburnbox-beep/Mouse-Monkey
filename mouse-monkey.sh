#!/usr/bin/env bash
# Start or stop Mouse Monkey.
#
#   mouse-monkey.sh           start him if he isn't running, otherwise send him home
#   mouse-monkey.sh start     start him (does nothing if he's already running)
#   mouse-monkey.sh stop      send him home
#
# Builds the app first if it hasn't been built yet or the code has changed since.
# The app menu entry created by install.sh runs this script.

set -u

# Resolve the project folder, even when run through a symlink in ~/.local/bin.
PROJECT_DIR="$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"
BIN="$PROJECT_DIR/target/release/monkey_companion"
RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp}"
PID_FILE="$RUNTIME_DIR/mouse-monkey.pid"
LOG_FILE="$RUNTIME_DIR/mouse-monkey.log"
ICON="$PROJECT_DIR/assets/icon.png"

# Launched from the app menu, the session's PATH may not include rustup's cargo.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

notify() {
    echo "Mouse Monkey: $1" >&2
    if [ ! -t 2 ] && command -v notify-send >/dev/null 2>&1; then
        notify-send --app-name="Mouse Monkey" --icon="$ICON" "Mouse Monkey" "$1" 2>/dev/null || true
    fi
}

# Prints the running monkey's PID, if there is one.
running_pid() {
    local pid
    pid="$(cat "$PID_FILE" 2>/dev/null)" || return 1
    # The kernel truncates process names to 15 characters.
    [ -n "$pid" ] && [ "$(cat "/proc/$pid/comm" 2>/dev/null)" = "monkey_companio" ] || return 1
    echo "$pid"
}

needs_build() {
    [ ! -x "$BIN" ] && return 0
    [ -n "$(find "$PROJECT_DIR/src" "$PROJECT_DIR/Cargo.toml" "$PROJECT_DIR/Cargo.lock" \
        -newer "$BIN" -print -quit 2>/dev/null)" ]
}

build() {
    if ! command -v cargo >/dev/null 2>&1; then
        notify "Can't build: Rust isn't installed. Install it from https://rustup.rs"
        return 1
    fi
    notify "Building... the first time takes a minute or two."
    if ! (cd "$PROJECT_DIR" && cargo build --release) >"$LOG_FILE" 2>&1; then
        notify "Build failed. Details are in $LOG_FILE"
        return 1
    fi
}

start() {
    if running_pid >/dev/null; then
        echo "Mouse Monkey is already running." >&2
        return 0
    fi
    if needs_build; then
        build || return 1
    fi

    # The config and sprite paths are relative to the project folder.
    cd "$PROJECT_DIR" || return 1
    setsid "$BIN" >"$LOG_FILE" 2>&1 </dev/null &
    local pid=$!
    echo "$pid" >"$PID_FILE"

    # Catch the case where he can't start at all (e.g. GNOME, no layer-shell).
    sleep 1
    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f "$PID_FILE"
        notify "He couldn't start. Details are in $LOG_FILE"
        return 1
    fi
    echo "Mouse Monkey started. Run this again to send him home." >&2
}

stop() {
    local pid
    if ! pid="$(running_pid)"; then
        rm -f "$PID_FILE"
        echo "Mouse Monkey isn't running." >&2
        return 0
    fi
    kill "$pid"
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.2
    done
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid"
    rm -f "$PID_FILE"
    echo "Mouse Monkey went home." >&2
}

case "${1:-toggle}" in
    start) start ;;
    stop) stop ;;
    toggle)
        if running_pid >/dev/null; then stop; else start; fi
        ;;
    *)
        echo "Usage: $0 [start|stop]" >&2
        exit 2
        ;;
esac
