#!/bin/sh
# Opens Herdr Planner in its own app window (not a browser tab); starts the
# systemd --user service when it is down.
#
#   herdr-planner-open [--window | --browser]
#   HERDR_PLANNER_OPEN=window|browser
#   HERDR_PLANNER_PORT / port in ~/.config/herdr-planner/config.toml (7117 default)
#   listen in the config: 127.0.0.1/0.0.0.0 use loopback, a named interface is used as is
#
# The window is a Chromium app window (Omarchy's web-app launcher), 1280×940.
# It uses its own profile (${XDG_DATA_HOME:-~/.local/share}/herdr-planner/chromium,
# profile "herdr-planner"), so its Wayland app-id is unique:
# chrome-…-herdr-planner. An existing window is found by that app-id with
# `hyprctl clients` and focused; a browser tab or an ordinary browser window is
# never matched, even when its title says "Herdr Planner". `--browser` is for
# debugging only.
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

# The profile name is also the window class marker: Chromium reports
# chrome-<host>-<profile> for an app window on Wayland, while ordinary browser
# windows of the same profile ("chromium", or "herdr-planner" with --class) do
# not get that app-id. Only the app window matches.
app_id=herdr-planner
profile_dir="${XDG_DATA_HOME:-$HOME/.local/share}/herdr-planner/chromium"

mode="${HERDR_PLANNER_OPEN:-window}"
case "${1:-}" in
  --window) mode=window ;;
  --browser) mode=browser ;;
  -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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

# Address of our own app window, if one is open. Matched by app-id only:
# chrome-…-herdr-planner (Chromium builds it from the URL host and the profile
# name). "herdr-planner" and "chromium" are ordinary browser windows in that
# profile and must not match; titles are deliberately not consulted.
find_window() {
  command -v hyprctl >/dev/null 2>&1 || return 1
  command -v jq >/dev/null 2>&1 || return 1
  hyprctl clients -j 2>/dev/null |
    jq -r --arg id "$app_id" '
      .[] | select(((.class // "") | ascii_downcase | endswith("-" + $id)))
      | .address' |
    head -n 1
}

# Focuses the window on its own workspace. hyprctl errors are reported with
# exit status 0, so the output is checked; older Hyprland has no Lua dispatcher
# and gets the legacy one.
focus_window() {
  addr="$1"
  out=$(hyprctl dispatch "hl.dsp.focus({ window = \"address:$addr\" })" 2>&1) || true
  case "$out" in ok*) return 0 ;; esac
  out=$(hyprctl dispatch focuswindow "address:$addr" 2>&1) || true
  case "$out" in ok*) return 0 ;; esac
  return 1
}

open_window() {
  set -- \
    --window-size=1280,940 \
    --user-data-dir="$profile_dir" \
    --profile-directory="$app_id" \
    --disable-extensions \
    --no-first-run \
    --no-default-browser-check
  mkdir -p "$profile_dir" 2>/dev/null || true
  if command -v omarchy-launch-webapp >/dev/null 2>&1; then
    omarchy-launch-webapp "$url" "$@" >/dev/null 2>&1 &
  else
    browser=
    for c in chromium google-chrome-stable brave brave-browser microsoft-edge-stable vivaldi-stable opera; do
      if command -v "$c" >/dev/null 2>&1; then browser=$c; break; fi
    done
    [ -n "$browser" ] || return 1
    setsid "$browser" --app="$url" "$@" >/dev/null 2>&1 &
  fi
  # A window handed to an already running browser can open in the background,
  # so wait for it and focus it. It is the only window with our app-id.
  n=0
  while [ "$n" -lt 24 ]; do
    addr=$(find_window) || addr=
    if [ -n "$addr" ] && focus_window "$addr"; then return 0; fi
    sleep 0.25
    n=$((n + 1))
  done
  return 0
}

open_browser() {
  command -v xdg-open >/dev/null 2>&1 || return 1
  setsid xdg-open "$url" >/dev/null 2>&1 &
}

if ! alive; then
  systemctl --user start herdr-planner.service >/dev/null 2>&1 || true
  n=0
  while [ "$n" -lt 40 ] && ! alive; do
    sleep 0.25
    n=$((n + 1))
  done
  alive || { say "the server does not answer at $url (systemctl --user status herdr-planner)"; exit 1; }
fi

case "$mode" in
  browser) open_browser || { say "no xdg-open"; exit 1; } ;;
  *)
    addr=$(find_window) || addr=
    if [ -n "$addr" ] && focus_window "$addr"; then exit 0; fi
    open_window || open_browser || { say "could not open $url"; exit 1; }
    ;;
esac
