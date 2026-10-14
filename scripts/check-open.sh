#!/bin/sh
# herdr-planner-open against stand-ins for herdr-gpui, the Omarchy launcher and xdg-open:
# a tab only when the running herdr-gpui has tabs, else the app window, and the system browser
# only on request. Never starts anything real.
#   scripts/check-open.sh [path/to/open.sh]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
OPEN="${1:-$here/open.sh}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT INT TERM
mkdir -p "$T/bin"
fake() { printf '#!/bin/sh\n%s\n' "$2" >"$T/bin/$1"; chmod +x "$T/bin/$1"; }
fake curl 'exit 0'
fake systemctl 'exit 0'
fake sleep 'exit 0'
fake omarchy-launch-or-focus-webapp 'echo "window $*" >>"$LOG"'
fake xdg-open 'echo "browser $*" >>"$LOG"'
# `browser text` is the probe, `browser open` the real thing; FAKE_TEXT_RC is what the probe exits with.
fake herdr-gpui 'case "$2" in text) echo "probe $*" >>"$LOG"; exit "${FAKE_TEXT_RC:-5}" ;; open) echo "tab $*" >>"$LOG" ;; *) exit 2 ;; esac'
fails=0
# run NAME EXPECTED_STATUS EXPECTED_LOG PROBE_EXIT [open.sh args…]: the log is the sorted non-probe calls.
run() {
  name="$1" want_status="$2" want_log="$3" probe="$4"; shift 4
  LOG="$T/log"; : >"$LOG"; export LOG
  set +e
  env HERDR_PLANNER_PORT=7117 HERDR_PLANNER_OPEN= FAKE_TEXT_RC="$probe" PATH="$T/bin:/usr/bin:/bin" sh "$OPEN" "$@" >"$T/out" 2>&1
  status=$?
  set -e
  sleep 0.4 2>/dev/null || true
  got="$(grep -v '^probe' "$LOG" | sort | tr '\n' '|')"
  if [ "$status" = "$want_status" ] && [ "$got" = "$want_log" ]; then echo "ok   $name"; else
    echo "FAIL $name: status $status (want $want_status), calls '$got' (want '$want_log')"; fails=$((fails + 1)); fi
}
URL="http://127.0.0.1:7117"
run "no tabs on this build (exit 8): the app window, not a tab or the browser" 0 "window Планировщик проектов $URL|" 8
run "tabs supported (exit 5, no such tab): a herdr-gpui tab" 0 "tab browser open localhost:7117|" 5
run "no Herdr UI running (exit 3): the app window" 0 "window Планировщик проектов $URL|" 3
run "--tab without tabs fails and opens nothing" 1 "" 8 --tab
run "--window goes to the window even with tabs" 0 "window Планировщик проектов $URL|" 5 --window
run "--browser is the only way to the system browser" 0 "browser $URL|" 8 --browser
rm -f "$T/bin/herdr-gpui"
run "no herdr-gpui at all: the app window" 0 "window Планировщик проектов $URL|" 0
[ "$fails" -eq 0 ]
