#!/usr/bin/env bash
# End-to-end check of save + apply in a nested shell:
# arrange known windows, save, scramble, apply, save again, compare.
#
#   scripts/smoke-apply.sh            (uses target/release/simple-session)
set -euo pipefail

cd "$(dirname "$0")/.."
BIN=${BIN:-$PWD/target/release/simple-session}
NESTED=scripts/nested.sh

export SIMPLE_SESSION_DIR
SIMPLE_SESSION_DIR=$(mktemp -d)
trap 'rm -rf "$SIMPLE_SESSION_DIR"; $NESTED stop' EXIT

$NESTED restart 1920x1080 1280x1024 >/dev/null

$NESTED exec bash -s "$BIN" <<'EOF'
set -euo pipefail
BIN=$1
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

# Scramble: everything onto workspace 0 and the primary monitor, unmaximized, visible.
for id in "${W[@]}"; do
    place "$id" "{\"workspace\":0,\"monitor\":0,\"rect\":{\"x\":$((RANDOM % 600)),\"y\":$((RANDOM % 300 + 40)),\"w\":400,\"h\":300}}"
done
sleep 0.5

echo "--- apply"
"$BIN" apply before
"$BIN" save after >/dev/null

python3 - "$SIMPLE_SESSION_DIR" <<'PY'
import json, sys
def load(name):
    s = json.load(open(f"{sys.argv[1]}/{name}.json"))
    rows = []
    for app in s["apps"]:
        for w in app["windows"]:
            r = w["rect_abs"]
            rect = None if w["maximized"] != "none" else (r["x"], r["y"], r["w"], r["h"])
            rows.append(((app["id"], w["match"]["title"], w["workspace"], w["monitor_key"],
                          w["maximized"], w["minimized"]), rect))
    return sorted(rows, key=str)
def same(b, a):
    # Geometry within 2 px (fractional scaling rounds).
    if b[1] is None or a[1] is None:
        return b == a
    return b[0] == a[0] and all(abs(p - q) <= 2 for p, q in zip(b[1], a[1]))
before, after = load("before"), load("after")
bad = [(b, a) for b, a in zip(before, after) if not same(b, a)]
for b, a in bad:
    print("MISMATCH\n  saved:  ", b, "\n  applied:", a)
print("--- smoke:", "FAIL" if bad or len(before) != len(after) else "OK", f"({len(before)} windows)")
sys.exit(1 if bad else 0)
PY
EOF
