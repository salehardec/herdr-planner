#!/bin/sh
# Installs Herdr Planner for the current user: no sudo, no system packages.
#
#   scripts/install.sh [install] [--no-build] [--no-service] [--no-hypr] [--no-shell] [--key "SUPER + ALT + P"]
#   scripts/install.sh uninstall [--no-service] [--no-hypr] [--no-shell]
#   scripts/install.sh status
#   scripts/install.sh password [--user NAME] [--listen ADDR]
#   scripts/install.sh tls --cert FILE --key FILE
#
# The repository lives on NTFS in this setup, where compiled files cannot run:
# the build always goes outside it, to ${XDG_CACHE_HOME:-~/.cache}/herdr-planner/target
# (override with HERDR_PLANNER_TARGET_DIR).
#
# What goes where:
#   ~/.local/bin/herdr-planner, herdr-planner-open
#   ~/.config/systemd/user/herdr-planner.service   (enabled, started)
#   ~/.local/share/applications/herdr-planner.desktop,
#   ~/.local/share/icons/hicolor/scalable/apps/herdr-planner.svg
#   ~/.config/hypr/herdr_planner.lua + one marked block in bindings.lua
#   ~/.config/omarchy/plugins/salehardec.herdr-planner/ + one entry in shell.json
#   ~/.config/herdr-planner/config.toml            (only `password` / `tls` write it)
#   ~/.local/state/herdr-planner/                  (journal, chat; kept on uninstall)
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
bin_dir="${HERDR_PLANNER_BIN_DIR:-$HOME/.local/bin}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
cache_home="${XDG_CACHE_HOME:-$HOME/.cache}"
target_dir="${HERDR_PLANNER_TARGET_DIR:-$cache_home/herdr-planner/target}"
unit_dir="$config_home/systemd/user"
hypr_dir="$config_home/hypr"
unit=herdr-planner.service
bin="$bin_dir/herdr-planner"
cfg="$config_home/herdr-planner/config.toml"
begin_mark='-- >>> herdr-planner (scripts/install.sh) >>>'
end_mark='-- <<< herdr-planner <<<'
plugin_id=salehardec.herdr-planner
omarchy_dir="$config_home/omarchy"
plugin_dir="$omarchy_dir/plugins/$plugin_id"
shell_json="$omarchy_dir/shell.json"

action=install
build=1
service=1
hypr=1
shell=1
key=
login_user=herdr
listen=
cert=
tls_key=
while [ $# -gt 0 ]; do
  case "$1" in
    install | uninstall | status | password | tls) action="$1" ;;
    --no-build) build=0 ;;
    --no-service) service=0 ;;
    --no-hypr) hypr=0 ;;
    --no-shell) shell=0 ;;
    --key)
      [ $# -ge 2 ] || { echo "install.sh: --key needs a value" >&2; exit 2; }
      if [ "$action" = tls ] || [ -n "$cert" ]; then tls_key="$2"; else key="$2"; fi
      shift ;;
    --user) [ $# -ge 2 ] || { echo "install.sh: --user needs a value" >&2; exit 2; }; login_user="$2"; shift ;;
    --listen) [ $# -ge 2 ] || { echo "install.sh: --listen needs a value" >&2; exit 2; }; listen="$2"; shift ;;
    --cert) [ $# -ge 2 ] || { echo "install.sh: --cert needs a value" >&2; exit 2; }; cert="$2"; shift ;;
    --key-file) [ $# -ge 2 ] || { echo "install.sh: --key-file needs a value" >&2; exit 2; }; tls_key="$2"; shift ;;
    -h | --help) sed -n '2,/^set -eu/{/^set -eu/!p;}' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '%s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# The shell's plugin list is reloaded only in a real Omarchy session.
shell_reload=0
if [ "$shell" -eq 1 ] && [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && have omarchy-shell; then
  shell_reload=1
fi

service_path() {
  p="$bin_dir"
  for tool in herdr-projects herdr claude; do
    t=$(command -v "$tool" 2>/dev/null || true)
    [ -n "$t" ] || continue
    d=$(dirname "$t")
    case ":$p:" in *":$d:"*) ;; *) p="$p:$d" ;; esac
  done
  printf '%s' "$p:/usr/local/bin:/usr/bin:/bin"
}

port() {
  p="${HERDR_PLANNER_PORT:-}"
  if [ -z "$p" ] && [ -r "$cfg" ]; then
    p=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$cfg" | head -n 1)
  fi
  printf '%s' "${p:-7117}"
}

listen_addr() {
  l=""
  if [ -r "$cfg" ]; then
    l=$(sed -n 's/^[[:space:]]*listen[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$cfg" | head -n 1)
  fi
  printf '%s' "${l:-127.0.0.1}"
}

# A URL this machine can use to reach the service, whatever it binds to:
# 0.0.0.0/:: become loopback, a specific interface stays itself, and TLS is
# used when the config names a certificate.
probe_url() {
  l=$(listen_addr)
  case "$l" in
    "" | 127.* | localhost | 0.0.0.0) h=127.0.0.1 ;;
    :: | ::1 | "[::1]" | "[::]") h="[::1]" ;;
    \[*) h="$l" ;;
    *:*) h="[$l]" ;;
    *) h="$l" ;;
  esac
  s=http
  if [ -r "$cfg" ] && grep -q '^[[:space:]]*tls_cert[[:space:]]*=' "$cfg"; then s=https; fi
  printf '%s://%s:%s' "$s" "$h" "$(port)"
}

build_bin() {
  [ "$build" -eq 1 ] || return 0
  cargo_cmd="cargo"
  cargo --version >/dev/null 2>&1 || cargo_cmd="mise x rust@stable -- cargo"
  say "Building release in $target_dir …"
  CARGO_TARGET_DIR="$target_dir" $cargo_cmd build --release
}

built_bin() {
  [ -x "$target_dir/release/herdr-planner" ] || { echo "install.sh: $target_dir/release/herdr-planner is missing: build first" >&2; exit 1; }
  printf '%s' "$target_dir/release/herdr-planner"
}

install_bin() {
  src="${HERDR_PLANNER_SRC:-$(built_bin)}"
  [ -x "$src" ] || { echo "install.sh: $src is missing: build first" >&2; exit 1; }
  mkdir -p "$bin_dir"
  install -m 755 "$src" "$bin_dir/.herdr-planner.new"
  mv -f "$bin_dir/.herdr-planner.new" "$bin"
  install -m 755 "$repo/scripts/open.sh" "$bin_dir/herdr-planner-open"
  say "Installed $bin ($("$bin" --version)) and herdr-planner-open"
}

ensure_bin() {
  if [ ! -x "$bin" ] || [ "$build" -eq 1 ]; then
    build_bin
    install_bin
  fi
}

# ---- systemd ----

install_service() {
  if [ "$service" -eq 0 ]; then return 0; fi
  if ! have systemctl; then say "systemctl not found: service skipped"; return 0; fi
  mkdir -p "$unit_dir"
  sed -e "s|@BIN@|$bin|" -e "s|@PATH@|$(service_path)|" "$repo/packaging/$unit" >"$unit_dir/$unit"
  systemctl --user daemon-reload
  systemctl --user enable "$unit" >/dev/null 2>&1
  if ! systemctl --user is-active --quiet "$unit" && pgrep -x herdr-planner >/dev/null 2>&1; then
    say "Note: a herdr-planner started by hand holds port $(port); stop it (pkill -x herdr-planner)."
  fi
  systemctl --user restart "$unit"
  i=0
  url=$(probe_url)
  while [ "$i" -lt 80 ] && ! curl -kfs --max-time 2 "$url/login" >/dev/null 2>&1; do
    sleep 0.25
    i=$((i + 1))
  done
  if [ "$i" -lt 80 ]; then
    say "Service: $unit on $url (journalctl --user -u herdr-planner)"
  else
    say "Service: $unit did not answer on port $(port): systemctl --user status herdr-planner"
    exit 1
  fi
}

restart_service() {
  if [ "$service" -eq 1 ] && have systemctl && systemctl --user is-enabled --quiet "$unit" 2>/dev/null; then
    systemctl --user restart "$unit" >/dev/null 2>&1 || true
  fi
}

# ---- Hyprland ----

in_hyprland() { [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && have hyprctl; }

key_parts() {
  printf '%s' "$1" | awk -F'[ +]+' '{
    m = 0; k = ""
    for (i = 1; i <= NF; i++) {
      t = toupper($i)
      if (t == "") continue
      if (t == "SUPER" || t == "MOD4" || t == "WIN") m += 64
      else if (t == "CTRL" || t == "CONTROL") m += 4
      else if (t == "ALT" || t == "MOD1") m += 8
      else if (t == "SHIFT") m += 1
      else k = t
    }
    print m, k }'
}

key_busy() {
  have jq || return 1
  in_hyprland || return 1
  set -- $(key_parts "$1")
  hyprctl -j binds 2>/dev/null | jq -e --argjson m "$1" --arg k "$2" '
    def bit($n): (. / $n | floor) % 2;
    any(.[]; ((.modmask | bit(64)) * 64 + (.modmask | bit(8)) * 8 + (.modmask | bit(4)) * 4 + (.modmask | bit(1))) as $x
      | $x == $m and (.key | ascii_upcase) == $k and .description != "Herdr Planner")' >/dev/null 2>&1
}

current_key() {
  [ -r "$hypr_dir/herdr_planner.lua" ] || return 0
  sed -n 's/^o\.bind("\([^"]*\)", "Herdr Planner".*/\1/p' "$hypr_dir/herdr_planner.lua" | head -n 1
}

pick_key() {
  [ -z "$key" ] || { printf '%s' "$key"; return; }
  cur=$(current_key)
  [ -z "$cur" ] || { printf '%s' "$cur"; return; }
  for cand in "SUPER + ALT + P" "SUPER + SHIFT + H" "SUPER + SHIFT + L" "SUPER + SHIFT + T"; do
    if ! key_busy "$cand"; then printf '%s' "$cand"; return; fi
  done
  return 1
}

hypr_install() {
  [ "$hypr" -eq 1 ] || return 0
  if [ ! -f "$hypr_dir/bindings.lua" ]; then
    say "Hyprland: $hypr_dir/bindings.lua not found: shortcut skipped"
    return 0
  fi
  chosen=$(pick_key) || { say "Hyprland: no free key among the candidates; pass --key \"SUPER + ...\""; return 0; }
  if [ -n "$key" ] && key_busy "$key"; then
    say "Hyprland: $key is already bound to something else; pick another with --key"
    return 1
  fi
  cat >"$hypr_dir/herdr_planner.lua" <<EOF
-- Written by Herdr Planner's scripts/install.sh; the installer rewrites this file.
-- Opens the dashboard in its own app window.
o.bind("$chosen", "Herdr Planner", "$bin_dir/herdr-planner-open")
EOF
  if ! grep -qF -- "$begin_mark" "$hypr_dir/bindings.lua"; then
    {
      printf '\n%s\n' "$begin_mark"
      printf '%s\n' 'pcall(require, "hypr.herdr_planner")'
      printf '%s\n' "$end_mark"
    } >>"$hypr_dir/bindings.lua"
  fi
  say "Hyprland: $chosen opens Herdr Planner"
  if in_hyprland; then hyprctl reload >/dev/null 2>&1 || true; fi
}

hypr_uninstall() {
  [ "$hypr" -eq 1 ] || return 0
  rm -f "$hypr_dir/herdr_planner.lua"
  if [ -f "$hypr_dir/bindings.lua" ] && grep -qF -- "$begin_mark" "$hypr_dir/bindings.lua"; then
    tmp=$(mktemp "$hypr_dir/.bindings.lua.XXXXXX")
    awk -v b="$begin_mark" -v e="$end_mark" '
      { line[NR] = $0 }
      END {
        for (i = 1; i <= NR; i++) {
          if (line[i] == b) {
            while (i <= NR && line[i] != e) i++
            if (out > 0 && last == "") out--
            continue
          }
          kept[++out] = line[i]
          last = line[i]
        }
        for (i = 1; i <= out; i++) print kept[i]
      }' "$hypr_dir/bindings.lua" >"$tmp"
    cat "$tmp" >"$hypr_dir/bindings.lua"
    rm -f "$tmp"
  fi
  if in_hyprland; then hyprctl reload >/dev/null 2>&1 || true; fi
}

# ---- Omarchy bar plugin ----

shell_edit() {
  filter="$1"; shift
  tmp=$(mktemp "$omarchy_dir/.shell.json.XXXXXX")
  if ! jq --arg id "$plugin_id" "$@" "$filter" "$shell_json" >"$tmp" 2>/dev/null; then
    rm -f "$tmp"; return 1
  fi
  if [ "$(jq -c . "$tmp")" = "$(jq -c . "$shell_json")" ]; then
    rm -f "$tmp"; echo same; return 0
  fi
  cp -p "$shell_json" "$shell_json.bak-herdr-planner-$(date +%s)"
  cat "$tmp" >"$shell_json"; rm -f "$tmp"; echo changed
}

filter_add='
  def mine: type == "object" and .id == $id;
  def setport: if $port == 0 then del(.port) else .port = $port end;
  if ([.bar.layout[]? | arrays | .[] | select(mine)] | length) > 0 then
    .bar.layout |= with_entries(.value |= (if type == "array" then map(if mine then setport else . end) else . end))
  else
    ({id: $id, chip: false} | setport) as $e
    | (.bar.centerAnchor // "omarchy.clock") as $anchor
    | .bar.layout.center as $c
    | ($c | map(if type == "object" then .id else . end) | index($anchor)) as $i
    | .bar.layout.center = (if $i == null then $c + [$e] else $c[:$i + 1] + [$e] + $c[$i + 1:] end)
  end'
filter_remove='
  def mine: type == "object" and .id == $id;
  (if (.bar.layout | type) == "object" then
     .bar.layout |= with_entries(.value |= (if type == "array" then map(select(mine | not)) else . end))
   else . end)
  | (if (.plugins | type) == "array" then .plugins |= map(select(mine | not)) else . end)
  | (if (.disabledPlugins | type) == "array" then .disabledPlugins |= map(select(. != $id)) else . end)'

shell_install() {
  [ "$shell" -eq 1 ] || return 0
  if [ ! -f "$shell_json" ]; then say "Omarchy shell: $shell_json not found: bar icon skipped"; return 0; fi
  if ! have jq; then say "Omarchy shell: jq not found: bar icon skipped"; return 0; fi
  if [ "$(jq -r '.bar.layout.center | type' "$shell_json" 2>/dev/null)" != array ]; then
    say "Omarchy shell: no bar layout in $shell_json: bar icon skipped"
    return 0
  fi
  src_dir="$repo/packaging/omarchy-plugin"
  mkdir -p "$plugin_dir"
  rm -f "$plugin_dir"/*.qml "$plugin_dir"/manifest.json
  install -m 644 "$src_dir/manifest.json" "$src_dir/Widget.qml" "$plugin_dir/"
  p=$(port); [ "$p" != 7117 ] || p=0
  r=$(shell_edit "$filter_add" --argjson port "$p") || { say "Omarchy shell: could not edit $shell_json"; return 1; }
  say "Omarchy shell: bar icon $r ($plugin_dir)"
  if [ "$shell_reload" -eq 1 ]; then omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; fi
}

shell_uninstall() {
  [ "$shell" -eq 1 ] || return 0
  if [ -f "$shell_json" ] && have jq; then shell_edit "$filter_remove" >/dev/null || true; fi
  if [ -f "$plugin_dir/manifest.json" ] && grep -q "\"id\": \"$plugin_id\"" "$plugin_dir/manifest.json"; then
    rm -rf "$plugin_dir"
  fi
  if [ "$shell_reload" -eq 1 ]; then omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; fi
}

# ---- actions ----

do_install() {
  build_bin
  install_bin
  mkdir -p "$data_home/applications" "$data_home/icons/hicolor/scalable/apps"
  sed "s|@OPEN@|$bin_dir/herdr-planner-open|" "$repo/packaging/herdr-planner.desktop" \
    >"$data_home/applications/herdr-planner.desktop"
  install -m 644 "$repo/packaging/herdr-planner.svg" "$data_home/icons/hicolor/scalable/apps/herdr-planner.svg"
  if have update-desktop-database; then update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true; fi
  say "Launcher: $data_home/applications/herdr-planner.desktop"
  install_service
  hypr_install
  shell_install
}

do_uninstall() {
  if [ "$service" -eq 1 ] && have systemctl; then
    systemctl --user disable --now "$unit" >/dev/null 2>&1 || true
    rm -f "$unit_dir/$unit"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  hypr_uninstall
  shell_uninstall
  rm -f "$bin" "$bin_dir/herdr-planner-open" "$bin_dir/.herdr-planner.new" \
    "$data_home/applications/herdr-planner.desktop" \
    "$data_home/icons/hicolor/scalable/apps/herdr-planner.svg"
  if have update-desktop-database; then update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true; fi
  say "Removed the binary, service, launcher, shortcut and bar icon."
  say "Kept: $cfg and ${XDG_STATE_HOME:-$HOME/.local/state}/herdr-planner"
}

do_status() {
  for f in "$bin" "$bin_dir/herdr-planner-open" "$unit_dir/$unit" \
    "$data_home/applications/herdr-planner.desktop" "$hypr_dir/herdr_planner.lua" "$plugin_dir/manifest.json"; do
    if [ -e "$f" ]; then say "ok       $f"; else say "missing  $f"; fi
  done
  if [ -f "$cfg" ]; then
    say "config   $cfg"
    u=herdr
    u=$(sed -n 's/^[[:space:]]*user[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$cfg" | head -n 1)
    [ -n "$u" ] || u=herdr
    if grep -q '^[[:space:]]*password_hash[[:space:]]*=' "$cfg"; then say "login    on ($u / $(listen_addr):$(port))"; else say "login    off (127.0.0.1 only)"; fi
    if grep -q '^[[:space:]]*tls_cert[[:space:]]*=' "$cfg"; then say "tls      on"; else say "tls      off"; fi
  else
    say "config   $cfg (not written: login off, 127.0.0.1)"
  fi
  if have systemctl; then
    say "service  enabled=$(systemctl --user is-enabled "$unit" 2>/dev/null || true) active=$(systemctl --user is-active "$unit" 2>/dev/null || true)"
  fi
  if [ -f "$shell_json" ] && have jq; then
    if jq -e --arg id "$plugin_id" 'any(.bar.layout[]?; type == "array" and any(.[]; type == "object" and .id == $id))' "$shell_json" >/dev/null 2>&1; then
      say "bar      $plugin_id is in the bar layout"
    else
      say "bar      $plugin_id is not in the bar layout"
    fi
  fi
  url=$(probe_url)
  if curl -kfs --max-time 2 "$url/login" >/dev/null 2>&1; then
    say "http     answers on $url"
  else
    say "http     no answer on $url"
  fi
}

do_password() {
  ensure_bin
  say "Пароль хранится только как Argon2id-хеш в $cfg."
  printf 'Пароль для «%s»: ' "$login_user"
  stty -echo 2>/dev/null || true
  IFS= read -r pw || true
  stty echo 2>/dev/null || true
  printf '\nЕщё раз: '
  stty -echo 2>/dev/null || true
  IFS= read -r pw2 || true
  stty echo 2>/dev/null || true
  printf '\n'
  [ -n "$pw" ] || { echo "install.sh: empty password" >&2; exit 2; }
  [ "$pw" = "$pw2" ] || { echo "install.sh: passwords do not match" >&2; exit 2; }
  hash=$(printf '%s' "$pw" | "$bin" hash-password) || { echo "install.sh: could not hash the password" >&2; exit 1; }
  if [ -n "$listen" ]; then
    printf '%s' "$hash" | "$bin" set-auth --user "$login_user" --listen "$listen"
  else
    printf '%s' "$hash" | "$bin" set-auth --user "$login_user"
  fi
  say "Login: $login_user on $(listen_addr):$(port)"
  if [ "$(listen_addr)" != "127.0.0.1" ] && ! grep -q '^[[:space:]]*tls_cert[[:space:]]*=' "$cfg" 2>/dev/null; then
    say "Note: the session cookie is Secure. For a remote browser use HTTPS"
    say "      (scripts/install.sh tls --cert FILE --key FILE) or a TLS proxy / Tailscale Serve."
  fi
  restart_service
}

do_tls() {
  ensure_bin
  [ -n "$cert" ] && [ -n "$tls_key" ] || { echo "install.sh tls: --cert FILE --key FILE are required" >&2; exit 2; }
  [ -r "$cert" ] || { echo "install.sh tls: cannot read $cert" >&2; exit 1; }
  [ -r "$tls_key" ] || { echo "install.sh tls: cannot read $tls_key" >&2; exit 1; }
  "$bin" set-tls --cert "$cert" --key "$tls_key"
  restart_service
}

case "$action" in
  install) do_install ;;
  uninstall) do_uninstall ;;
  status) do_status ;;
  password) do_password ;;
  tls) do_tls ;;
esac
