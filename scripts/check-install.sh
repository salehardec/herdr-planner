#!/bin/sh
# Checks scripts/install.sh and scripts/open.sh without touching the real home:
# every path points into a temporary folder, no Hyprland reload, and `systemctl` is a
# stand-in that only records its arguments (the real user service has the same name,
# so a temporary XDG_CONFIG_HOME alone would not keep it safe).
#   scripts/check-install.sh [path-to-binary]
set -eu
cd "$(dirname "$0")/.."
BIN="$(cd "$(dirname "${1:-target/release/herdr-planner}")" && pwd)/$(basename "${1:-target/release/herdr-planner}")"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/cfg/hypr" "$T/fakebin"
printf '#!/bin/sh\necho "$*" >>"%s/systemctl.log"\n' "$T" >"$T/fakebin/systemctl"
chmod +x "$T/fakebin/systemctl"
: >"$T/systemctl.log"
printf -- '-- mine\no.bind("X", "x", "y")\n' >"$T/cfg/hypr/bindings.lua"
cp "$T/cfg/hypr/bindings.lua" "$T/orig"

for f in scripts/install.sh scripts/open.sh scripts/check-install.sh scripts/run-checks.sh; do sh -n "$f"; done
if command -v shellcheck >/dev/null 2>&1; then shellcheck scripts/*.sh; fi

inst() {
  env -u HYPRLAND_INSTANCE_SIGNATURE PATH="$T/fakebin:$PATH" XDG_CONFIG_HOME="$T/cfg" XDG_DATA_HOME="$T/data" \
    HERDR_PLANNER_BIN_DIR="$T/bin" HERDR_PLANNER_SRC="$BIN" sh scripts/install.sh "$@"
}
# Anything but a read-only query of the service.
mutated() { grep -E '^--user (enable|disable|restart|start|stop|daemon-reload|reload)' "$T/systemctl.log" || true; }
fail() { echo "check-install: $*" >&2; exit 1; }

inst install --no-build --no-service >/dev/null
inst install --no-build --no-service >/dev/null
[ -z "$(mutated)" ] || fail "--no-service must not change the service: $(mutated)"
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
inst uninstall --no-service >/dev/null
[ -z "$(mutated)" ] || fail "uninstall --no-service must not change the service: $(mutated)"
cmp -s "$T/orig" "$T/cfg/hypr/bindings.lua" || fail "uninstall must restore bindings.lua byte for byte"
[ ! -e "$T/bin/herdr-planner" ] && [ ! -e "$T/cfg/hypr/herdr_planner.lua" ] || fail "uninstall left files"
# The service path, against the stand-in only: it must enable and restart, and uninstall must disable.
inst install --no-build --no-hypr >/dev/null 2>&1 || true
grep -q '^--user enable herdr-planner.service' "$T/systemctl.log" || fail "install must enable the unit (stand-in log: $(cat "$T/systemctl.log"))"
grep -q '^--user restart herdr-planner.service' "$T/systemctl.log" || fail "install must restart the unit"
[ -f "$T/cfg/systemd/user/herdr-planner.service" ] || fail "unit file missing in the temporary config dir"
inst uninstall --no-hypr >/dev/null
grep -q '^--user disable --now herdr-planner.service' "$T/systemctl.log" || fail "uninstall must disable the unit"
[ ! -e "$T/cfg/systemd/user/herdr-planner.service" ] || fail "uninstall left the unit file"
echo "check-install: ok"
