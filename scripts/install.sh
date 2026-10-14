#!/bin/sh
# Installs Herdr Planner for the current user: no sudo, no system package manager.
#   scripts/install.sh [install] [--no-build] [--no-service] [--no-hypr] [--key "SUPER + ALT + P"]
#   scripts/install.sh uninstall      removes everything installed; state and config stay
#   scripts/install.sh status
# Running install again updates: it rebuilds, replaces the binary and restarts the service.
#
# What goes where:
#   ~/.local/bin/herdr-planner, herdr-planner-open
#   ~/.config/systemd/user/herdr-planner.service        (enabled, started)
#   ~/.local/share/applications/herdr-planner.desktop, ~/.local/share/icons/hicolor/scalable/apps/herdr-planner.svg
#   ~/.config/hypr/herdr_planner.lua + one marked block in ~/.config/hypr/bindings.lua   (Omarchy, Lua config)
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
bin_dir="${HERDR_PLANNER_BIN_DIR:-$HOME/.local/bin}"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
unit_dir="$config_home/systemd/user"
hypr_dir="$config_home/hypr"
unit=herdr-planner.service
begin_mark='-- >>> herdr-planner (scripts/install.sh) >>>'
end_mark='-- <<< herdr-planner <<<'

action=install
build=1
service=1
hypr=1
key=
while [ $# -gt 0 ]; do
  case "$1" in
    install | uninstall | status) action="$1" ;;
    --no-build) build=0 ;;
    --no-service) service=0 ;;
    --no-hypr) hypr=0 ;;
    --key)
      [ $# -ge 2 ] || { echo "install.sh: --key needs a value" >&2; exit 2; }
      key="$2"
      shift
      ;;
    -h | --help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '%s\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# Systemd user units do not see the login shell's PATH: give the service the directories
# of the tools the planner starts.
service_path() {
  p="$bin_dir"
  for tool in herdr-projects herdr claude git; do
    t=$(command -v "$tool" 2>/dev/null || true)
    [ -n "$t" ] || continue
    d=$(dirname "$t")
    case ":$p:" in *":$d:"*) ;; *) p="$p:$d" ;; esac
  done
  printf '%s' "$p:/usr/local/bin:/usr/bin:/bin"
}

port() {
  p="${HERDR_PLANNER_PORT:-}"
  if [ -z "$p" ] && [ -r "$config_home/herdr-planner/config.toml" ]; then
    p=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$config_home/herdr-planner/config.toml" | head -n 1)
  fi
  printf '%s' "${p:-7117}"
}

# ---- Hyprland (Omarchy Lua config) ----

in_hyprland() { [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && have hyprctl; }

# "SUPER + ALT + P" -> "72 P" (Hyprland modmask: SHIFT 1, CTRL 4, ALT 8, SUPER 64).
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

# Succeeds when the key is bound to something other than Herdr Planner.
key_busy() {
  have jq || return 1
  in_hyprland || return 1
  set -- $(key_parts "$1")
  # jq has no bit operations: pull out SHIFT, CTRL, ALT and SUPER by division.
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
  for cand in "SUPER + ALT + P" "SUPER + SHIFT + H" "SUPER + SHIFT + L" "SUPER + SHIFT + T" "SUPER + SHIFT + Z"; do
    if ! key_busy "$cand"; then printf '%s' "$cand"; return; fi
  done
  return 1
}

hypr_install() {
  if [ ! -f "$hypr_dir/bindings.lua" ]; then
    say "Hyprland: $hypr_dir/bindings.lua not found (not an Omarchy Lua config): shortcut skipped"
    return 0
  fi
  chosen=$(pick_key) || { say "Hyprland: no free key among the candidates; pass --key \"SUPER + ...\""; return 0; }
  if [ -n "$key" ] && key_busy "$key"; then
    say "Hyprland: $key is already bound to something else; pick another with --key"
    return 1
  fi
  cat >"$hypr_dir/herdr_planner.lua" <<EOF
-- Written by Herdr Planner's scripts/install.sh; the installer rewrites this file.
-- Opens the planner: a herdr-gpui tab, otherwise an Omarchy window or the system browser.
o.bind("$chosen", "Herdr Planner", "$bin_dir/herdr-planner-open")
EOF
  if ! grep -qF -- "$begin_mark" "$hypr_dir/bindings.lua"; then
    {
      printf '\n%s\n' "$begin_mark"
      printf '%s\n' 'pcall(require, "hypr.herdr_planner")'
      printf '%s\n' "$end_mark"
    } >>"$hypr_dir/bindings.lua"
  fi
  say "Hyprland: $chosen opens Herdr Planner ($hypr_dir/herdr_planner.lua)"
  if in_hyprland; then hyprctl reload >/dev/null 2>&1 || true; fi
}

hypr_uninstall() {
  rm -f "$hypr_dir/herdr_planner.lua"
  if [ -f "$hypr_dir/bindings.lua" ] && grep -qF -- "$begin_mark" "$hypr_dir/bindings.lua"; then
    tmp=$(mktemp "$hypr_dir/.bindings.lua.XXXXXX")
    # Drops the marked block and the blank line the installer put before it.
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

# ---- install / uninstall / status ----

do_install() {
  if [ "$build" -eq 1 ]; then
    cd "$repo"
    cargo_cmd="cargo"
    cargo --version >/dev/null 2>&1 || cargo_cmd="mise x rust@stable -- cargo"
    say "Building release ($cargo_cmd build --release)…"
    $cargo_cmd build --release
  fi
  src="${HERDR_PLANNER_SRC:-$repo/target/release/herdr-planner}"
  [ -x "$src" ] || { echo "install.sh: $src is missing: build first" >&2; exit 1; }

  mkdir -p "$bin_dir"
  # Copy beside, then rename: a running binary cannot be overwritten in place.
  install -m 755 "$src" "$bin_dir/.herdr-planner.new"
  mv -f "$bin_dir/.herdr-planner.new" "$bin_dir/herdr-planner"
  install -m 755 "$repo/scripts/open.sh" "$bin_dir/herdr-planner-open"
  say "Installed $bin_dir/herdr-planner ($("$bin_dir/herdr-planner" --version)) and herdr-planner-open"

  mkdir -p "$data_home/applications" "$data_home/icons/hicolor/scalable/apps"
  sed "s|@OPEN@|$bin_dir/herdr-planner-open|" "$repo/packaging/herdr-planner.desktop" \
    >"$data_home/applications/herdr-planner.desktop"
  install -m 644 "$repo/packaging/herdr-planner.svg" "$data_home/icons/hicolor/scalable/apps/herdr-planner.svg"
  if have update-desktop-database; then update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true; fi
  say "Launcher: $data_home/applications/herdr-planner.desktop"

  if [ "$service" -eq 1 ]; then
    if have systemctl; then
      mkdir -p "$unit_dir"
      sed -e "s|@BIN@|$bin_dir/herdr-planner|" -e "s|@PATH@|$(service_path)|" \
        "$repo/packaging/$unit" >"$unit_dir/$unit"
      systemctl --user daemon-reload
      systemctl --user enable "$unit" >/dev/null 2>&1
      # A planner started by hand holds the port: the service could not bind it.
      if ! systemctl --user is-active --quiet "$unit" && pgrep -x herdr-planner >/dev/null 2>&1; then
        say "Note: a herdr-planner started by hand is running and holds port $(port): stop it (pkill -x herdr-planner)."
      fi
      systemctl --user restart "$unit"
      i=0
      while [ "$i" -lt 40 ] && ! curl -fs --max-time 2 "http://127.0.0.1:$(port)/api/snapshot" >/dev/null 2>&1; do
        sleep 0.25
        i=$((i + 1))
      done
      if [ "$i" -lt 40 ]; then
        say "Service: $unit is running on http://127.0.0.1:$(port) (journalctl --user -u herdr-planner)"
      else
        say "Service: $unit did not answer on port $(port): systemctl --user status herdr-planner"
        exit 1
      fi
    else
      say "systemctl not found: service skipped"
    fi
  fi

  if [ "$hypr" -eq 1 ]; then hypr_install; fi
}

do_uninstall() {
  if have systemctl; then
    systemctl --user disable --now "$unit" >/dev/null 2>&1 || true
    rm -f "$unit_dir/$unit"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  hypr_uninstall
  rm -f "$bin_dir/herdr-planner" "$bin_dir/herdr-planner-open" "$bin_dir/.herdr-planner.new" \
    "$data_home/applications/herdr-planner.desktop" \
    "$data_home/icons/hicolor/scalable/apps/herdr-planner.svg"
  if have update-desktop-database; then update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true; fi
  say "Removed the binary, service, launcher and shortcut."
  say "Kept: $config_home/herdr-planner and ${XDG_STATE_HOME:-$HOME/.local/state}/herdr-planner (config, groups, answers)."
}

do_status() {
  for f in "$bin_dir/herdr-planner" "$bin_dir/herdr-planner-open" "$unit_dir/$unit" \
    "$data_home/applications/herdr-planner.desktop" "$hypr_dir/herdr_planner.lua"; do
    if [ -e "$f" ]; then say "ok       $f"; else say "missing  $f"; fi
  done
  if have systemctl; then
    say "service  enabled=$(systemctl --user is-enabled "$unit" 2>/dev/null || true) active=$(systemctl --user is-active "$unit" 2>/dev/null || true)"
  fi
  k=$(current_key)
  [ -z "$k" ] || say "key      $k"
  if curl -fs --max-time 2 "http://127.0.0.1:$(port)/api/snapshot" >/dev/null 2>&1; then
    say "http     answers on 127.0.0.1:$(port)"
  else
    say "http     no answer on 127.0.0.1:$(port)"
  fi
}

case "$action" in
  install) do_install ;;
  uninstall) do_uninstall ;;
  status) do_status ;;
esac
