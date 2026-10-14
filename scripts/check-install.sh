#!/bin/sh
# Checks scripts/install.sh and scripts/open.sh without touching the real home:
# every path points into a temporary folder, no service and no Hyprland reload.
#   scripts/check-install.sh [path-to-binary]
set -eu
cd "$(dirname "$0")/.."
BIN="$(cd "$(dirname "${1:-target/release/herdr-planner}")" && pwd)/$(basename "${1:-target/release/herdr-planner}")"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/cfg/hypr"
printf -- '-- mine\no.bind("X", "x", "y")\n' >"$T/cfg/hypr/bindings.lua"
cp "$T/cfg/hypr/bindings.lua" "$T/orig"

for f in scripts/install.sh scripts/open.sh scripts/check-install.sh scripts/run-checks.sh; do sh -n "$f"; done
if command -v shellcheck >/dev/null 2>&1; then shellcheck scripts/*.sh; fi

inst() {
  env -u HYPRLAND_INSTANCE_SIGNATURE XDG_CONFIG_HOME="$T/cfg" XDG_DATA_HOME="$T/data" \
    HERDR_PLANNER_BIN_DIR="$T/bin" HERDR_PLANNER_SRC="$BIN" sh scripts/install.sh "$@"
}
fail() { echo "check-install: $*" >&2; exit 1; }

inst install --no-build --no-service >/dev/null
inst install --no-build --no-service >/dev/null
[ -x "$T/bin/herdr-planner" ] && [ -x "$T/bin/herdr-planner-open" ] || fail "binaries missing"
grep -q "^Exec=$T/bin/herdr-planner-open" "$T/data/applications/herdr-planner.desktop" || fail "launcher Exec"
[ -f "$T/data/icons/hicolor/scalable/apps/herdr-planner.svg" ] || fail "icon missing"
[ "$(grep -c '>>> herdr-planner' "$T/cfg/hypr/bindings.lua")" -eq 1 ] || fail "bindings.lua block duplicated"
grep -q "\"SUPER + ALT + P\", \"Herdr Planner\", \"$T/bin/herdr-planner-open\"" "$T/cfg/hypr/herdr_planner.lua" || fail "binding line"
inst install --no-build --no-service --key "SUPER + SHIFT + H" >/dev/null
grep -q '"SUPER + SHIFT + H"' "$T/cfg/hypr/herdr_planner.lua" || fail "--key"
inst install --no-build --no-service >/dev/null
grep -q '"SUPER + SHIFT + H"' "$T/cfg/hypr/herdr_planner.lua" || fail "reinstall must keep the chosen key"
inst status >/dev/null
inst uninstall >/dev/null
cmp -s "$T/orig" "$T/cfg/hypr/bindings.lua" || fail "uninstall must restore bindings.lua byte for byte"
[ ! -e "$T/bin/herdr-planner" ] && [ ! -e "$T/cfg/hypr/herdr_planner.lua" ] || fail "uninstall left files"
echo "check-install: ok"
