#!/bin/sh
# Opens Herdr Planner in its own app window (not a browser tab); starts the
# systemd --user service when it is down.
#
#   herdr-planner-open [--window | --browser]
#   HERDR_PLANNER_OPEN=window|browser
#   HERDR_PLANNER_PORT / port in ~/.config/herdr-planner/config.toml (7117 default)
#   listen in the config: 127.0.0.1/0.0.0.0 use loopback, a named interface is used as is
#
# The window is a Chromium app window (Omarchy's web-app launcher), 1280×940,
# focused if it is already open. `--browser` is for debugging only.
set -u

cfg="${XDG_CONFIG_HOME:-$HOME/.config}/herdr-planner/config.toml"
port="${HERDR_PLANNER_PORT:-}"
if [ -z "$port" ] && [ -r "$cfg" ]; then
  port=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$cfg" | head -n 1)
fi
port="${port:-7117}"
listen=
if [ -r "$cfg" ]; then
  listen=$(sed -n 's/^[[:space:]]*listen[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$cfg" | head -n 1)
fi
case "$listen" in
  "" | 127.* | localhost | 0.0.0.0) host=127.0.0.1 ;;
  :: | ::1 | "[::1]" | "[::]") host="[::1]" ;;
  \[*) host="$listen" ;;
  *:*) host="[$listen]" ;;
  *) host="$listen" ;;
esac
scheme=http
if [ -r "$cfg" ] && grep -q '^[[:space:]]*tls_cert[[:space:]]*=' "$cfg"; then scheme=https; fi
url="$scheme://$host:$port"

mode="${HERDR_PLANNER_OPEN:-window}"
case "${1:-}" in
  --window) mode=window ;;
  --browser) mode=browser ;;
  -h | --help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  "") ;;
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

alive() { curl -fs -k --max-time 2 "$url/login" >/dev/null 2>&1; }

if ! alive; then
  systemctl --user start herdr-planner.service >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 40 ] && ! alive; do
    sleep 0.25
    n=$((n + 1))
  done
  alive || { say "the server does not answer at $url (systemctl --user status herdr-planner)"; exit 1; }
fi

open_window() {
  if command -v omarchy-launch-or-focus-webapp >/dev/null 2>&1; then
    omarchy-launch-or-focus-webapp "Herdr Planner" "$url" --window-size=1280,940 >/dev/null 2>&1 &
  elif command -v omarchy-launch-webapp >/dev/null 2>&1; then
    omarchy-launch-webapp "$url" --window-size=1280,940 >/dev/null 2>&1 &
  else
    return 1
  fi
}

open_browser() {
  command -v xdg-open >/dev/null 2>&1 || return 1
  setsid xdg-open "$url" >/dev/null 2>&1 &
}

case "$mode" in
  browser) open_browser || { say "no xdg-open"; exit 1; } ;;
  *) open_window || open_browser || { say "could not open $url"; exit 1; } ;;
esac
