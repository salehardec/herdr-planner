#!/bin/sh
# herdr-planner-open against stand-ins for the Omarchy launchers and xdg-open:
# an app window by default, the browser only when asked, and the service is
# started first when it does not answer.
#   scripts/check-open.sh [path/to/open.sh]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
OPEN="${1:-$here/open.sh}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/bin" "$T/sys" "$T/cfg/herdr-planner"
for c in sed head grep; do ln -s "$(command -v "$c")" "$T/sys/$c"; done
fake() { printf '#!/bin/sh\n%s\n' "$2" >"$T/bin/$1"; chmod +x "$T/bin/$1"; }
fake curl 'exit 0'
fake systemctl 'echo "$*" >>"$LOG"'
fake sleep 'exit 0'
fake setsid 'exec "$@"'
fake omarchy-launch-or-focus-webapp 'echo "window $*" >>"$LOG"'
fake omarchy-launch-webapp 'echo "webapp $*" >>"$LOG"'
fake xdg-open 'echo "browser $*" >>"$LOG"'
fails=0
run() {
  name="$1" want_status="$2" want_log="$3"; shift 3
  LOG="$T/log"; : >"$LOG"; export LOG
  set +e
  env HERDR_PLANNER_PORT= HERDR_PLANNER_OPEN= XDG_CONFIG_HOME="$T/cfg" PATH="$T/bin:$T/sys" /bin/sh "$OPEN" "$@" >"$T/out" 2>&1
  status=$?
  set -e
  sleep 0.3 2>/dev/null || true
  got="$(sort "$LOG" | tr '\n' '|')"
  if [ "$status" = "$want_status" ] && [ "$got" = "$want_log" ]; then echo "ok   $name"; else
    echo "FAIL $name: status $status (want $want_status), calls '$got' (want '$want_log')"; fails=$((fails + 1)); fi
}
URL="http://127.0.0.1:7117"
run "default: one app window, 1280x940, no browser" 0 "window Herdr Planner $URL --window-size=1280,940|"
run "--browser is the only way to the system browser" 0 "browser $URL|" --browser
rm -f "$T/bin/omarchy-launch-or-focus-webapp"
run "without the launcher: omarchy-launch-webapp" 0 "webapp $URL --window-size=1280,940|"
rm -f "$T/bin/omarchy-launch-webapp"
run "without any Omarchy launcher: xdg-open" 0 "browser $URL|"
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
