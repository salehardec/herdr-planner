#!/bin/sh
# herdr-planner-open against stand-ins for Hyprland, the Omarchy launchers and
# xdg-open: an app window by default, the browser only when asked, the service
# started first when it does not answer, and an existing planner window found
# by its app-id (never by title; ordinary browser windows do not match).
#   scripts/check-open.sh [path/to/open.sh]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
OPEN="${1:-$here/open.sh}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/bin" "$T/sys" "$T/cfg/herdr-planner"
for c in sed head grep mkdir jq; do
  p=$(command -v "$c")
  [ -x "$p" ] || p="/usr/bin/$c"
  ln -s "$p" "$T/sys/$c"
done
fake() { printf '#!/bin/sh\n%s\n' "$2" >"$T/bin/$1"; chmod +x "$T/bin/$1"; }
fake curl 'exit 0'
fake systemctl 'echo "$*" >>"$LOG"'
fake sleep '/usr/bin/sleep 0.01'
fake setsid 'exec "$@"'
fake hyprctl 'case "$1" in
  clients) if [ -f "$LAUNCHED" ]; then printf "%s\n" "$CLIENTS_AFTER"; else printf "%s\n" "$CLIENTS"; fi ;;
  dispatch) echo "focus $*" >>"$LOG"; echo ok ;;
  *) exit 0 ;;
esac'
fake omarchy-launch-webapp 'echo "window $*" >>"$LOG"
: >"$LAUNCHED"'
fake chromium 'echo "chromium $*" >>"$LOG"
: >"$LAUNCHED"'
fake xdg-open 'echo "browser $*" >>"$LOG"'
fails=0
run() {
  name="$1" want_status="$2" want_log="$3"; shift 3
  LOG="$T/log"; : >"$LOG"; export LOG
  rm -f "$T/launched"
  set +e
  env HERDR_PLANNER_PORT= HERDR_PLANNER_OPEN= XDG_CONFIG_HOME="$T/cfg" \
    XDG_DATA_HOME="$T/data" CLIENTS="${CLIENTS:-[]}" \
    CLIENTS_AFTER="${CLIENTS_AFTER:-[]}" LAUNCHED="$T/launched" \
    PATH="$T/bin:$T/sys" /bin/sh "$OPEN" "$@" >"$T/out" 2>&1
  status=$?
  set -e
  # The launches go on in the background after open.sh has returned: wait until the log is what it should be (at most 10 s on
  # a loaded machine, at once on a free one), then a moment more to catch a call that should not have been made.
  tries=0
  while [ "$(sort "$LOG" | tr '\n' '|')" != "$want_log" ] && [ "$tries" -lt 100 ]; do
    tries=$((tries + 1)); sleep 0.1 2>/dev/null || true
  done
  sleep 0.3 2>/dev/null || true
  got="$(sort "$LOG" | tr '\n' '|')"
  if [ "$status" = "$want_status" ] && [ "$got" = "$want_log" ]; then echo "ok   $name"; else
    echo "FAIL $name: status $status (want $want_status), calls '$got' (want '$want_log')"; fails=$((fails + 1)); fi
}
URL="http://127.0.0.1:7117"
FLAGS="--window-size=1280,940 --user-data-dir=$T/data/herdr-planner/chromium --profile-directory=herdr-planner --disable-extensions --no-first-run --no-default-browser-check"
focus() { printf 'focus dispatch hl.dsp.focus({ window = "address:%s" })|' "$1"; }

CLIENTS='[]'
run "default: one app window, 1280x940, separate profile" 0 "window $URL $FLAGS|"

CLIENTS='[]'
CLIENTS_AFTER='[{"class":"chrome-127.0.0.1__-herdr-planner","title":"Herdr Planner","address":"0xnew","workspace":{"id":5}}]'
run "a newly opened app window is focused" 0 "$(focus 0xnew)window $URL $FLAGS|"
CLIENTS_AFTER='[]'

CLIENTS='[{"class":"yandex-browser","title":"Herdr Planner, группа AI_Providers — Яндекс Браузер","address":"0xbrowser","workspace":{"id":11}}]'
run "a browser tab titled Herdr Planner is not our window" 0 "window $URL $FLAGS|"

CLIENTS='[{"class":"yandex-browser","title":"Herdr Planner","address":"0xbrowser","workspace":{"id":11}},{"class":"herdr-planner","title":"Herdr Planner – Chromium","address":"0xnormal","workspace":{"id":11}},{"class":"chromium","title":"Herdr Planner – Chromium","address":"0xchromium","workspace":{"id":11}},{"class":"chrome-127.0.0.1__-herdr-planner","title":"Herdr Planner","address":"0xown","workspace":{"id":5}}]'
run "our app window: focus it, not the browser windows of its profile" 0 "$(focus 0xown)"

CLIENTS='[{"class":"herdr-planner","title":"Herdr Planner – Chromium","address":"0xnormal","workspace":{"id":11}},{"class":"chromium","title":"Herdr Planner – Chromium","address":"0xchromium","workspace":{"id":11}}]'
run "ordinary browser windows in the planner profile are not ours" 0 "window $URL $FLAGS|"

CLIENTS='[{"class":"chrome-127.0.0.1__-herdr-planner-other","title":"Herdr Planner","address":"0xother","workspace":{"id":5}}]'
run "a class that merely contains the marker is not ours" 0 "window $URL $FLAGS|"

CLIENTS='[{"class":"chrome-127.0.0.1__-herdr-planner","title":"Herdr Planner","address":"0xown","workspace":{"id":5}}]'
run "--browser is the only way to the system browser" 0 "browser $URL|" --browser

rm -f "$T/bin/omarchy-launch-webapp"
CLIENTS='[]'
run "without the launcher: Chromium directly" 0 "chromium --app=$URL $FLAGS|"
rm -f "$T/bin/chromium"
run "without any app browser: xdg-open" 0 "browser $URL|"
printf 'listen = "127.0.0.1"\nport = 7333\ntls_cert = "/tmp/c"\ntls_key = "/tmp/k"\n' >"$T/cfg/herdr-planner/config.toml"
run "port and TLS from config: https on 7333" 0 "browser https://127.0.0.1:7333|"
printf 'listen = "192.0.2.5"\nport = 7333\n' >"$T/cfg/herdr-planner/config.toml"
run "a named interface is used as is" 0 "browser http://192.0.2.5:7333|"
printf 'listen = "0.0.0.0"\nport = 7333\n' >"$T/cfg/herdr-planner/config.toml"
run "0.0.0.0 opens loopback" 0 "browser http://127.0.0.1:7333|"
printf 'listen = "::1"\nport = 7444\n' >"$T/cfg/herdr-planner/config.toml"
run "IPv6 loopback gets brackets" 0 "browser http://[::1]:7444|"
rm -f "$T/cfg/herdr-planner/config.toml"
[ "$fails" -eq 0 ]
