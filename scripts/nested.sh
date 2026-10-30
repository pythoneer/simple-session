#!/usr/bin/env bash
# Run a nested GNOME Shell (mutter-devkit) for testing the shim without
# logging out. The nested shell gets its own D-Bus session and in-memory
# GSettings, so it cannot touch the real session's state or settings.
# Extension code is loaded at shell start, so `restart` picks up edits.
#
#   scripts/nested.sh start [WxH ...]   start in background (default: 1920x1080)
#   scripts/nested.sh stop
#   scripts/nested.sh restart [WxH ...]
#   scripts/nested.sh env               print exports for the nested session
#   scripts/nested.sh exec <cmd...>     run a command inside the nested session
#   scripts/nested.sh eval <js>         evaluate JS in the nested shell (Shell.Eval)
#
# Example:
#   scripts/nested.sh start
#   scripts/nested.sh exec scripts/shim.sh windows
#   scripts/nested.sh exec xterm
set -euo pipefail

UUID=simple-session@dbensing.de
STATE_DIR=${XDG_RUNTIME_DIR:-/tmp}/simple-session-nested

running() {
    [[ -f $STATE_DIR/shell.pid ]] && kill -0 "$(cat "$STATE_DIR/shell.pid")" 2>/dev/null
}

load_env() {
    running || { echo "nested session not running" >&2; exit 1; }
    export DBUS_SESSION_BUS_ADDRESS
    DBUS_SESSION_BUS_ADDRESS=$(cat "$STATE_DIR/bus")
    # Same session store as the top-bar menu in the nested shell, unless the
    # caller chose one (smoke tests use a temporary directory).
    export SIMPLE_SESSION_DIR=${SIMPLE_SESSION_DIR:-$STATE_DIR/sessions}
    # Display name is chosen by mutter (passing --wayland-display fails in
    # devkit mode), so read it from the log.
    WAYLAND_DISPLAY=$(sed -n "s/.*Using Wayland display name '\([^']*\)'.*/\1/p" "$STATE_DIR/log" | tail -1)
    DISPLAY=$(sed -n 's/.*Using public X11 display \(:[0-9]*\).*/\1/p' "$STATE_DIR/log" | tail -1)
    export WAYLAND_DISPLAY DISPLAY
    # Xwayland starts on demand, but mutter writes its auth file at shell
    # start: take the newest one created after the nested shell started.
    local started f
    started=$(stat -c %Y "/proc/$(cat "$STATE_DIR/shell.pid")")
    unset XAUTHORITY
    for f in $(ls -t "$XDG_RUNTIME_DIR"/.mutter-Xwaylandauth.* 2>/dev/null); do
        if (( $(stat -c %Y "$f") >= started )); then
            export XAUTHORITY=$f
            break
        fi
    done
}

start() {
    if running; then echo "already running"; return; fi
    mkdir -p "$STATE_DIR"
    local monitors=("$@") args=()
    [[ ${#monitors[@]} -eq 0 ]] && monitors=(1920x1080)
    for m in "${monitors[@]}"; do args+=(--virtual-monitor "$m"); done

    # The menu runs the CLI from the repo and keeps sessions apart from the
    # real ones; unsafe mode lets tests drive the menu via Shell.Eval.
    SIMPLE_SESSION_CLI=$(cd "$(dirname "$0")/.." && pwd)/target/release/simple-session \
    SIMPLE_SESSION_DIR=$STATE_DIR/sessions \
    SIMPLE_SESSION_NESTED_UNSAFE=1 \
    setsid dbus-run-session -- bash -c '
        state=$1; shift
        echo "$DBUS_SESSION_BUS_ADDRESS" > "$state/bus"
        GSETTINGS_BACKEND=memory gnome-shell --devkit --wayland "$@" &
        echo $! > "$state/shell.pid"
        wait
    ' _ "$STATE_DIR" "${args[@]}" >"$STATE_DIR/log" 2>&1 &

    local enabled=
    for _ in $(seq 150); do
        if [[ -f $STATE_DIR/bus ]] && running; then
            export DBUS_SESSION_BUS_ADDRESS
            DBUS_SESSION_BUS_ADDRESS=$(cat "$STATE_DIR/bus")
            if [[ -z $enabled ]] && gdbus call --session --dest org.gnome.Shell \
                --object-path /org/gnome/Shell \
                --method org.gnome.Shell.Extensions.EnableExtension "$UUID" >/dev/null 2>&1; then
                enabled=1
            fi
            if [[ -n $enabled ]] && gdbus call --session --dest org.gnome.Shell \
                --object-path /de/dbensing/SimpleSession/Shim \
                --method de.dbensing.SimpleSession.Shim1.GetVersion >/dev/null 2>&1; then
                echo "started, shim ready (log: $STATE_DIR/log)"
                return
            fi
        fi
        sleep 0.2
    done
    echo "shell did not come up, see $STATE_DIR/log" >&2
    exit 1
}

stop() {
    if running; then
        local pid
        pid=$(cat "$STATE_DIR/shell.pid")
        kill "$pid"
        while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    fi
    rm -f "$STATE_DIR/shell.pid" "$STATE_DIR/bus"
}

cmd=${1:-}
shift || true
case $cmd in
    start) start "$@" ;;
    stop) stop ;;
    restart) stop; start "$@" ;;
    env)
        load_env
        for v in DBUS_SESSION_BUS_ADDRESS WAYLAND_DISPLAY DISPLAY XAUTHORITY; do
            [[ -n ${!v:-} ]] && echo "export $v='${!v}'"
        done
        ;;
    exec) load_env; exec "$@" ;;
    eval)
        load_env
        gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
            --method org.gnome.Shell.Eval "$1"
        ;;
    *) sed -n '2,17p' "$0"; exit 1 ;;
esac
