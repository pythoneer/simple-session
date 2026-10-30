#!/usr/bin/env bash
# End-to-end check in a nested shell: arrange known windows, save, then
#   apply:   scramble the windows and run `apply`
#   restore: close all apps and run `restore`
# and finally save again and compare with the first save.
#
#   scripts/smoke.sh [apply|restore]   (uses target/release/simple-session)
set -euo pipefail

MODE=${1:-apply}
[[ $MODE == apply || $MODE == restore ]] || { echo "usage: $0 [apply|restore]" >&2; exit 2; }

cd "$(dirname "$0")/.."
BIN=${BIN:-$PWD/target/release/simple-session}
NESTED=scripts/nested.sh

export SIMPLE_SESSION_DIR
SIMPLE_SESSION_DIR=$(mktemp -d)
trap 'rm -rf "$SIMPLE_SESSION_DIR"; $NESTED stop' EXIT

$NESTED restart 1920x1080 1280x1024 >/dev/null

$NESTED exec bash -s "$BIN" "$MODE" <<'EOF'
set -euo pipefail
BIN=$1
MODE=$2
for cmd in "alacritty -T one" "alacritty -T two" "alacritty -T three" gedit xterm gnome-calculator; do
    $cmd >/dev/null 2>&1 &
done
sleep 3

ids() { scripts/shim.sh windows | python3 -c "import json,sys; print(' '.join(w['id'] for w in sorted(json.load(sys.stdin), key=lambda w: w['stable_seq'])))"; }
read -r -a W <<<"$(ids)"
[[ ${#W[@]} -eq 6 ]] || { echo "expected 6 windows, got ${#W[@]}"; exit 1; }

monitors=$(scripts/shim.sh monitors)
second_x=$(python3 -c "import json,sys; m=json.loads(sys.argv[1]); print([x for x in m if not x['primary']][0]['x'])" "$monitors")

place() { scripts/shim.sh place "$1" "$2" >/dev/null; }
arrange() {
    place "${W[0]}" '{"workspace":0,"rect":{"x":100,"y":100,"w":700,"h":500}}'
    place "${W[1]}" '{"workspace":1,"rect":{"x":300,"y":200,"w":800,"h":600}}'
    place "${W[2]}" '{"workspace":1,"rect":{"x":900,"y":150,"w":600,"h":400}}'
    place "${W[3]}" '{"workspace":2,"rect":{"x":200,"y":300,"w":900,"h":600}}'
    place "${W[5]}" '{"workspace":0,"rect":{"x":1200,"y":350,"w":420,"h":616}}'
    place "${W[4]}" "{\"monitor\":1,\"rect\":{\"x\":$((second_x + 50)),\"y\":60,\"w\":500,\"h\":400}}"
}
# Geometry twice (the first pass may lose positions to async resizes; that is
# what apply has to cope with, but the reference must be exact), then states
# once the sizes are committed.
arrange; sleep 0.5; arrange; sleep 0.5
scripts/shim.sh state "${W[2]}" '{"minimized":true}' >/dev/null
scripts/shim.sh state "${W[3]}" '{"maximized":"both"}' >/dev/null
sleep 0.5

"$BIN" save before >/dev/null

if [[ $MODE == apply ]]; then
    # Scramble: everything onto workspace 0 and the primary monitor, unmaximized, visible.
    for id in "${W[@]}"; do
        place "$id" "{\"workspace\":0,\"monitor\":0,\"rect\":{\"x\":$((RANDOM % 600)),\"y\":$((RANDOM % 300 + 40)),\"w\":400,\"h\":300}}"
    done
    sleep 0.5
    echo "--- apply"
    "$BIN" apply before
else
    kill $(jobs -p)
    for _ in $(seq 50); do
        [[ $(scripts/shim.sh windows) == "[]" ]] && break
        sleep 0.1
    done
    echo "--- restore"
    "$BIN" restore before
fi
"$BIN" save after >/dev/null

python3 - "$SIMPLE_SESSION_DIR" "$MODE" <<'PY'
import json, sys
# Relaunched apps do not get their old titles back (alacritty -T), so only
# apply compares titles.
with_titles = sys.argv[2] == "apply"
def load(name):
    s = json.load(open(f"{sys.argv[1]}/{name}.json"))
    rows = []
    for app in s["apps"]:
        for w in app["windows"]:
            r = w["rect_abs"]
            rect = None if w["maximized"] != "none" else (r["x"], r["y"], r["w"], r["h"])
            rows.append(((app["id"], w["match"]["title"] if with_titles else None, w["workspace"], w["monitor_key"],
                          w["maximized"], w["minimized"]), rect))
    return sorted(rows, key=str)
def same(b, a):
    # Position within 2 px (fractional scaling rounds). Size too, except after
    # restore: a relaunched app may have a slightly different minimum size
    # (gedit's depends on its title).
    if b[1] is None or a[1] is None:
        return b == a
    size_tolerance = 2 if with_titles else 8
    (x, y, w, h), (x2, y2, w2, h2) = b[1], a[1]
    return (b[0] == a[0] and abs(x - x2) <= 2 and abs(y - y2) <= 2
            and abs(w - w2) <= size_tolerance and abs(h - h2) <= size_tolerance)
before, after = load("before"), load("after")
bad = [(b, a) for b, a in zip(before, after) if not same(b, a)]
for b, a in bad:
    print("MISMATCH\n  saved:  ", b, "\n  applied:", a)
print("--- smoke:", "FAIL" if bad or len(before) != len(after) else "OK", f"({len(before)} windows)")
sys.exit(1 if bad else 0)
PY
EOF
