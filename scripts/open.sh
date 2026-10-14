#!/bin/sh
# Opens Herdr Planner: a tab of herdr-gpui first, otherwise an Omarchy app window,
# otherwise the system browser. Starts the systemd --user service if it is down.
#   herdr-planner-open [--tab | --window | --browser]
# HERDR_PLANNER_OPEN=tab|window|browser does the same; HERDR_PLANNER_PORT / port in
# ~/.config/herdr-planner/config.toml pick the port (7117 by default).
# On the Linux build of herdr-gpui, which has no browser tabs, `browser open` itself
# sends the page to the system browser and exits 0: that counts as opened.
set -u

port="${HERDR_PLANNER_PORT:-}"
if [ -z "$port" ]; then
  cfg="${XDG_CONFIG_HOME:-$HOME/.config}/herdr-planner/config.toml"
  [ -r "$cfg" ] && port=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$cfg" | head -n 1)
fi
port="${port:-7117}"
url="http://127.0.0.1:$port"
mode="${HERDR_PLANNER_OPEN:-auto}"
[ $# -eq 0 ] || case "$1" in
  --tab) mode=tab ;;
  --window) mode=window ;;
  --browser) mode=browser ;;
  -h | --help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "herdr-planner-open: unknown option $1" >&2; exit 2 ;;
esac

say() {
  echo "herdr-planner-open: $*" >&2
  if command -v omarchy-notification-send >/dev/null 2>&1; then
    omarchy-notification-send -u low "Herdr Planner: $*" >/dev/null 2>&1 || true
  elif command -v notify-send >/dev/null 2>&1; then
    notify-send "Herdr Planner" "$*" >/dev/null 2>&1 || true
  fi
}

alive() { curl -fs --max-time 2 "$url/api/snapshot" >/dev/null 2>&1; }

if ! alive; then
  systemctl --user start herdr-planner.service >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 40 ] && ! alive; do
    sleep 0.25
    n=$((n + 1))
  done
  alive || { say "the server does not answer at $url (systemctl --user status herdr-planner)"; exit 1; }
fi

open_tab() {
  command -v herdr-gpui >/dev/null 2>&1 || return 1
  herdr-gpui browser open "localhost:$port"
}

open_window() {
  if command -v omarchy-launch-or-focus-webapp >/dev/null 2>&1; then
    # The window is found by the page title (see web/index.html).
    omarchy-launch-or-focus-webapp "Планировщик проектов" "$url" >/dev/null 2>&1 &
  elif command -v omarchy-launch-webapp >/dev/null 2>&1; then
    omarchy-launch-webapp "$url" >/dev/null 2>&1 &
  else
    return 1
  fi
}

open_browser() {
  command -v xdg-open >/dev/null 2>&1 || return 1
  setsid xdg-open "$url" >/dev/null 2>&1 &
}

case "$mode" in
  tab) open_tab ;;
  window) open_window || open_browser ;;
  browser) open_browser ;;
  *) open_tab || open_window || open_browser ;;
esac
status=$?
[ "$status" -eq 0 ] || say "could not open $url; open it in any browser"
exit "$status"
