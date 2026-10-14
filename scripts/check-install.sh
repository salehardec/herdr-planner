#!/bin/sh
# Checks scripts/install.sh and scripts/open.sh without touching the real home:
# every path points into a temporary folder, no Hyprland reload, no shell restart, and `systemctl` is a
# stand-in that only records its arguments (the real user service has the same name,
# so a temporary XDG_CONFIG_HOME alone would not keep it safe).
#   scripts/check-install.sh [path-to-binary]
set -eu
cd "$(dirname "$0")/.."
BIN="$(cd "$(dirname "${1:-target/release/herdr-planner}")" && pwd)/$(basename "${1:-target/release/herdr-planner}")"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/cfg/hypr" "$T/cfg/omarchy" "$T/fakebin"
printf '#!/bin/sh\necho "$*" >>"%s/systemctl.log"\n' "$T" >"$T/fakebin/systemctl"
chmod +x "$T/fakebin/systemctl"
: >"$T/systemctl.log"
printf -- '-- mine\no.bind("X", "x", "y")\n' >"$T/cfg/hypr/bindings.lua"
cp "$T/cfg/hypr/bindings.lua" "$T/orig"
# A shell.json shaped like the user's (written by jq, so a round trip is byte for byte): the clock is
# the centre anchor, other widgets stand around it.
cat >"$T/shell.in" <<'EOF2'
{"bar":{"centerAnchor":"my.clock","layout":{"center":[{"id":"omarchy.indicators"},{"id":"my.clock","chip":false},{"id":"other.pulse"}],"left":[{"id":"a.menu"}],"right":["b.tray"]},"position":"top"},"plugins":[{"id":"lock.explorer","design":"x"}],"version":1}
EOF2
jq . "$T/shell.in" >"$T/cfg/omarchy/shell.json"
cp "$T/cfg/omarchy/shell.json" "$T/shell.orig"

command -v jq >/dev/null 2>&1 || { echo "check-install: jq is needed" >&2; exit 1; }
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
# The bar icon: copied plugin, one entry right after the clock, nothing else changed, a backup kept.
P="$T/cfg/omarchy/plugins/salehardec.herdr-planner"
[ -f "$P/manifest.json" ] && [ -f "$P/Widget.qml" ] || fail "bar plugin not copied"
[ "$(jq -c '[.bar.layout.center[] | .id]' "$T/cfg/omarchy/shell.json")" = '["omarchy.indicators","my.clock","salehardec.herdr-planner","other.pulse"]' ] || fail "bar entry is not right after the clock: $(jq -c .bar.layout.center "$T/cfg/omarchy/shell.json")"
[ "$(jq -c '.bar.layout.center[2]' "$T/cfg/omarchy/shell.json")" = '{"id":"salehardec.herdr-planner","chip":false}' ] || fail "bar entry shape"
[ "$(jq -c 'del(.bar.layout.center[2])' "$T/cfg/omarchy/shell.json")" = "$(jq -c . "$T/shell.orig")" ] || fail "install changed more of shell.json than its own entry"
[ "$(grep -c salehardec.herdr-planner "$T/cfg/omarchy/shell.json")" -eq 1 ] || fail "bar entry duplicated"
[ "$(ls "$T/cfg/omarchy" | grep -c '^shell.json.bak-herdr-planner-')" -eq 1 ] || fail "expected exactly one shell.json backup, the second install changes nothing"
cmp -s "$T/shell.orig" "$(ls "$T/cfg/omarchy"/shell.json.bak-herdr-planner-* | head -n 1)" || fail "backup is not the original shell.json"
inst status | grep -q 'is in the bar layout' || fail "status must report the bar entry"
# A non-default port goes into the entry and back out.
HERDR_PLANNER_PORT=7555 inst install --no-build --no-service >/dev/null
[ "$(jq -c '.bar.layout.center[2]' "$T/cfg/omarchy/shell.json")" = '{"id":"salehardec.herdr-planner","chip":false,"port":7555}' ] || fail "port in the bar entry"
inst install --no-build --no-service >/dev/null
[ "$(jq -c '.bar.layout.center[2]' "$T/cfg/omarchy/shell.json")" = '{"id":"salehardec.herdr-planner","chip":false}' ] || fail "default port must drop the key"
# A user's own tweaks of the entry survive an update.
jq '.bar.layout.center[2].chip = "highlight"' "$T/cfg/omarchy/shell.json" >"$T/edit" && cat "$T/edit" >"$T/cfg/omarchy/shell.json"
inst install --no-build --no-service >/dev/null
[ "$(jq -r '.bar.layout.center[2].chip' "$T/cfg/omarchy/shell.json")" = highlight ] || fail "update must keep the user's chip setting"
jq '.bar.layout.center[2].chip = false' "$T/cfg/omarchy/shell.json" >"$T/edit" && cat "$T/edit" >"$T/cfg/omarchy/shell.json"
# --no-shell leaves both the plugin folder and shell.json alone.
inst uninstall --no-service --no-hypr --no-shell >/dev/null
[ -f "$P/manifest.json" ] && grep -q salehardec.herdr-planner "$T/cfg/omarchy/shell.json" || fail "--no-shell must not remove the bar icon"
inst install --no-build --no-service >/dev/null
inst uninstall --no-service >/dev/null
[ ! -e "$P" ] || fail "uninstall left the bar plugin"
[ "$(jq -S -c . "$T/cfg/omarchy/shell.json")" = "$(jq -S -c . "$T/shell.orig")" ] || fail "uninstall must give shell.json back as it was"
cmp -s "$T/shell.orig" "$T/cfg/omarchy/shell.json" || fail "uninstall must restore shell.json byte for byte"
# Someone else's folder with the same name is not ours to remove.
mkdir -p "$P" && printf '{"id":"somebody.else"}' >"$P/manifest.json"
inst uninstall --no-service >/dev/null
[ -f "$P/manifest.json" ] || fail "uninstall removed a plugin folder that is not ours"
rm -rf "$P"
# No bar layout in shell.json: the icon is skipped, shell.json untouched.
printf '{"version":1}\n' >"$T/cfg/omarchy/shell.json"
inst install --no-build --no-service >/dev/null
[ ! -e "$P" ] && [ "$(cat "$T/cfg/omarchy/shell.json")" = '{"version":1}' ] || fail "no layout: nothing may be written"
cp "$T/shell.orig" "$T/cfg/omarchy/shell.json"
inst install --no-build --no-service --no-shell >/dev/null
[ ! -e "$P" ] && cmp -s "$T/shell.orig" "$T/cfg/omarchy/shell.json" || fail "--no-shell install must not touch the shell"
inst uninstall --no-service >/dev/null
inst install --no-build --no-service >/dev/null
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
