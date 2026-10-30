#!/usr/bin/env bash
# Phase 0 helper: call the shim over D-Bus without the CLI.
#
#   scripts/shim.sh version
#   scripts/shim.sh windows | monitors | workspaces
#   scripts/shim.sh place <id> '{"monitor":1,"rect":{"x":100,"y":100,"w":800,"h":600}}'
#   scripts/shim.sh state <id> '{"maximized":"both"}'
#   scripts/shim.sh ensure <n>
#   scripts/shim.sh launch <desktop-id> [workspace]
#   scripts/shim.sh run [workspace] -- <argv...>
#   scripts/shim.sh watch            # WindowReady / WindowClosed signals
set -euo pipefail

DEST=org.gnome.Shell
OBJ=/de/dbensing/SimpleSession/Shim
IFACE=de.dbensing.SimpleSession.Shim1

call() {
    gdbus call --session --dest "$DEST" --object-path "$OBJ" --method "$IFACE.$1" "${@:2}"
}

# Unwrap a single GVariant string result ('...',) and pretty-print the JSON.
json() {
    python3 -c 'import ast,json,sys; print(json.dumps(json.loads(ast.literal_eval(sys.stdin.read())[0]), indent=2))'
}

cmd=${1:-}
shift || true
case "$cmd" in
    version)    call GetVersion ;;
    windows)    call ListWindows | json ;;
    monitors)   call ListMonitors | json ;;
    workspaces) call GetWorkspaces | json ;;
    place)      call PlaceWindow "$1" "$2" ;;
    state)      call SetWindowState "$1" "$2" ;;
    ensure)     call EnsureWorkspaces "$1" ;;
    launch)     call LaunchApp "$1" "${2:--1}" ;;
    run)
        ws=-1
        if [[ ${1:-} != -- ]]; then ws=$1; shift; fi
        shift # --
        argv=$(python3 -c 'import sys; print("[" + ", ".join(repr(a) for a in sys.argv[1:]) + "]")' "$@")
        call LaunchCommand "$argv" "$ws"
        ;;
    watch)
        gdbus monitor --session --dest "$DEST" --object-path "$OBJ" ;;
    *)
        sed -n '2,13p' "$0"
        exit 1
        ;;
esac
