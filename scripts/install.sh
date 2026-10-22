#!/bin/sh
# Herdr Planner for the current user: no root, system packages only when missing.
# Idempotent: run it again to update. Works from a clone (sh scripts/install.sh) and
# on its own (the release asset install.sh is this file).
#
#   sh install.sh [install] [flags]     install or update (step 1 to 7 below)
#   sh install.sh status                what is installed, where, and the address
#   sh install.sh uninstall             the planner only: herdr and the plugin stay
#   sh install.sh password [--user NAME] [--listen ADDR] [--password-stdin | --password-file FILE]
#   sh install.sh tls --cert FILE --key FILE
#
# Steps: [1/7] platform, [2/7] packages, [3/7] herdr (mise), [4/7] herdr-projects plugin,
# [5/7] herdr-planner (release archive, checksum checked; --from-source builds the clone),
# [6/7] service and launcher, [7/7] login and address.
#
# Release archive: ${HERDR_PLANNER_RELEASES:-https://github.com/salehardec/herdr-planner/releases}
#   /latest/download/ or /download/vX.Y.Z/ + herdr-planner-<linux|macos>-<x86_64|aarch64>.tar.gz
#   and its .sha256. It is unpacked to ~/.local/share/herdr-planner/release/<version>/.
#
# What goes where:
#   ~/.local/bin/herdr-planner, herdr-planner-open, herdr-projects (symlink)
#   ~/.local/share/herdr-planner/release/<version>/   (the unpacked archive)
#   Linux:   ~/.config/systemd/user/herdr-planner.service (enabled, started)
#   macOS:   ~/Library/LaunchAgents/dev.herdr.planner.plist (bootstrapped; log in ~/Library/Logs)
#   Omarchy: ~/.local/share/applications/herdr-planner.desktop and icon, a Hyprland key,
#            the bar icon in ~/.config/omarchy (skipped with "не Omarchy" elsewhere)
#   ~/.config/herdr-planner/config.toml     (only the login and TLS steps write it)
#   ~/.profile, ~/.zprofile                 (PATH block for login shells, between markers)
#   ~/.local/state/herdr-planner/           (journal, chat; kept on uninstall)
set -eu

releases=${HERDR_PLANNER_RELEASES:-https://github.com/salehardec/herdr-planner/releases}
herdr_min=0.9.3
plugin_repo=eliasstravik/herdr-projects
plugin_name=herdr-projects
label=dev.herdr.planner
unit=herdr-planner.service
plugin_id=salehardec.herdr-planner
hypr_begin='-- >>> herdr-planner (scripts/install.sh) >>>'
hypr_end='-- <<< herdr-planner <<<'
path_begin='# >>> herdr-planner PATH (scripts/install.sh) >>>'
path_end='# <<< herdr-planner PATH <<<'
archive_files='herdr-planner VERSION README.md scripts/install.sh scripts/open.sh
packaging/herdr-planner.service packaging/herdr-planner.desktop packaging/herdr-planner.svg
packaging/dev.herdr.planner.plist packaging/omarchy-plugin/manifest.json packaging/omarchy-plugin/Widget.qml'

say() { printf '%s\n' "$*"; }
res() { say "    $*"; }
dry_say() { say "    (dry-run) $*"; }
fail() { say "install.sh: $*" >&2; exit 1; }
stop() { say "install.sh: $*" >&2; exit 2; }
have() { command -v "$1" >/dev/null 2>&1; }
step() { say ""; say "[$1/7] $2"; }
run() {
  if [ "$dry" -eq 1 ]; then dry_say "$*"; return 0; fi
  "$@"
}

usage() {
  cat <<'EOF'
Herdr Planner: установка для текущего пользователя (без root; системные пакеты только недостающие).

  sh install.sh [install] [флаги]     установить или обновить (повторный запуск безопасен)
  sh install.sh status                что стоит, где и по какому адресу
  sh install.sh uninstall             снять планировщик (herdr и плагин не трогаются)
  sh install.sh password [--user NAME] [--listen ADDR] [--password-stdin | --password-file FILE]
  sh install.sh tls --cert FILE --key FILE

Флаги:
  --yes                 не спрашивать: ставить недостающие пакеты (через sudo) и плагин herdr-projects
  --dry-run             напечатать шаги, ничего не менять
  --version vX.Y.Z      версия релиза (по умолчанию latest)
  --from-source         собрать из клона (scripts/web-build.sh и cargo), а не брать релиз
  --no-herdr --no-plugin --no-service
  --user NAME --listen ADDR --password-stdin --password-file FILE
  --no-hypr --no-shell --key "SUPER + ALT + P"      только Omarchy

Без клона одной строкой:
  curl -fsSL https://github.com/salehardec/herdr-planner/releases/latest/download/install.sh | sh
EOF
}

# ---- arguments and platform ----

parse_args() {
  action=install; yes=0; dry=0; version=latest; from_source=0
  want_herdr=1; want_plugin=1; want_service=1; hypr=1; shell=1
  login_user=; listen=; pw_source=; pw_file=; cert=; key_arg=
  while [ $# -gt 0 ]; do
    case "$1" in
      install | uninstall | status | password | tls) action=$1 ;;
      --yes | -y) yes=1 ;;
      --dry-run) dry=1 ;;
      --from-source) from_source=1 ;;
      --no-herdr) want_herdr=0 ;;
      --no-plugin) want_plugin=0 ;;
      --no-service) want_service=0 ;;
      --no-hypr) hypr=0 ;;
      --no-shell) shell=0 ;;
      --password-stdin) pw_source=stdin ;;
      --version | --user | --listen | --password-file | --cert | --key | --key-file)
        [ $# -ge 2 ] || stop "$1 нужен аргумент"
        case "$1" in
          --version) version=$2 ;;
          --user) login_user=$2 ;;
          --listen) listen=$2 ;;
          --password-file) pw_source='file'; pw_file=$2 ;;
          --cert) cert=$2 ;;
          *) key_arg=$2 ;;
        esac
        shift ;;
      -h | --help) usage; exit 0 ;;
      *) stop "неизвестный аргумент: $1 (см. --help)" ;;
    esac
    shift
  done
  case "$version" in
    latest) ;;
    v[0-9]*.[0-9]*.[0-9]*) ;;
    [0-9]*.[0-9]*.[0-9]*) version=v$version ;;
    *) stop "--version: нужна версия вида v0.4.0 или latest" ;;
  esac
}

setup_env() {
  case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "платформа $(uname -s) не поддерживается: нужны Linux или macOS" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "архитектура $(uname -m) не поддерживается: нужны x86_64 или aarch64" ;;
  esac
  data_home=${XDG_DATA_HOME:-$HOME/.local/share}
  config_home=${XDG_CONFIG_HOME:-$HOME/.config}
  cache_home=${XDG_CACHE_HOME:-$HOME/.cache}
  state_home=${XDG_STATE_HOME:-$HOME/.local/state}
  bin_dir=${HERDR_PLANNER_BIN_DIR:-$HOME/.local/bin}
  target_dir=${HERDR_PLANNER_TARGET_DIR:-$cache_home/herdr-planner/target}
  rel_root=$data_home/herdr-planner/release
  unit_dir=$config_home/systemd/user
  hypr_dir=$config_home/hypr
  omarchy_dir=$config_home/omarchy
  plugin_dir=$omarchy_dir/plugins/$plugin_id
  shell_json=$omarchy_dir/shell.json
  bin=$bin_dir/herdr-planner
  cfg=$config_home/herdr-planner/config.toml
  plist=$HOME/Library/LaunchAgents/$label.plist
  PATH=$bin_dir:$HOME/.local/share/mise/shims:$PATH
  export PATH
  # The source tree when this file runs from a clone (scripts/install.sh next to Cargo.toml).
  src_root=
  if [ -f "$0" ] && [ -f "$(dirname "$0")/../Cargo.toml" ]; then
    src_root=$(cd "$(dirname "$0")/.." && pwd)
  fi
  ver=; rel=; bin_src=; mise_bin=
  planner_changed=0; unit_changed=0; omarchy=0; shell_reload=0
  if [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && have omarchy-shell; then shell_reload=1; fi
  hypr_key=$key_arg
  pw=; ans=
}

cleanup() {
  if [ -n "$work" ]; then rm -rf "$work"; fi
}

# Reads from the terminal even when the script itself came through a pipe.
ask_ok() { [ -t 0 ] || (exec </dev/tty) 2>/dev/null; }
read_tty() {
  if [ -t 0 ]; then IFS= read -r ans || ans=; else IFS= read -r ans </dev/tty || ans=; fi
}
ask_yes_no() { # prompt, default y|n; success means yes
  printf '%s ' "$1"
  read_tty
  case "$ans" in
    y | Y | yes | да | Да) return 0 ;;
    n | N | no | нет | Нет) return 1 ;;
    "") [ "$2" = y ] ;;
    *) return 1 ;;
  esac
}
read_secret() { # into pw, without echo
  if [ -t 0 ]; then
    stty -echo 2>/dev/null || true
    IFS= read -r pw || pw=
    stty echo 2>/dev/null || true
  else
    stty -echo </dev/tty 2>/dev/null || true
    IFS= read -r pw </dev/tty || pw=
    stty echo </dev/tty 2>/dev/null || true
  fi
  printf '\n'
}

# ---- config.toml (written by the binary; read here for the address and the login) ----

cfg_str() { # key -> its quoted value
  [ -r "$cfg" ] || return 0
  awk -F'"' -v k="$1" '$0 ~ ("^[[:space:]]*" k "[[:space:]]*=") { print $2; exit }' "$cfg"
}
has_cfg() { [ -r "$cfg" ] && grep -q "^[[:space:]]*$1[[:space:]]*=" "$cfg"; }
port() {
  p=${HERDR_PLANNER_PORT:-}
  if [ -z "$p" ] && [ -r "$cfg" ]; then
    p=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$cfg" | head -n 1)
  fi
  printf '%s' "${p:-7117}"
}
listen_addr() {
  l=$(cfg_str listen)
  printf '%s' "${l:-127.0.0.1}"
}
is_loopback() {
  case "$(listen_addr)" in
    127.* | localhost | ::1 | "[::1]") return 0 ;;
    *) return 1 ;;
  esac
}
scheme() { if has_cfg tls_cert; then printf https; else printf http; fi; }
# A URL this machine can use to reach the service, whatever it binds to:
# 0.0.0.0/:: become loopback, a specific interface stays itself.
probe_url() {
  l=$(listen_addr)
  case "$l" in
    127.* | localhost | 0.0.0.0) h=127.0.0.1 ;;
    :: | ::1 | "[::1]" | "[::]") h="[::1]" ;;
    \[*) h="$l" ;;
    *:*) h="[$l]" ;;
    *) h="$l" ;;
  esac
  printf '%s://%s:%s' "$(scheme)" "$h" "$(port)"
}
login_name() {
  if [ -n "$login_user" ]; then printf '%s' "$login_user"; return 0; fi
  u=$(cfg_str user)
  printf '%s' "${u:-herdr}"
}
host_name() { hostname -s 2>/dev/null || uname -n; }

# ---- versions ----

herdr_version() {
  h=$(command -v herdr 2>/dev/null) || return 1
  "$h" --version 2>/dev/null | sed -n 's/.*\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' | head -n 1
}
# $1 >= $2 for dotted versions
version_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]; }
hp_bin() {
  for c in "$bin_dir/herdr-projects" "$HOME/.herdr-projects/data/bin/herdr-projects"; do
    if [ -x "$c" ]; then printf '%s' "$c"; return 0; fi
  done
  command -v herdr-projects 2>/dev/null
}
hp_version() {
  h=$(hp_bin) || return 1
  "$h" version 2>/dev/null | head -n 1 | sed 's/^herdr-projects //; s/ (.*//'
}
bin_version() { "$bin" --version 2>/dev/null | awk '{print $2}'; }
sha256_of() {
  if have sha256sum; then sha256sum "$1" | awk '{print $1}'
  elif have shasum; then shasum -a 256 "$1" | awk '{print $1}'
  else fail "нет sha256sum и shasum"
  fi
}

# ---- step 1: platform ----

step_platform() {
  step 1 "платформа"
  res "$os $arch"
}

# ---- step 2: system packages (only the missing ones) ----

step_packages() {
  step 2 "пакеты"
  missing=
  for c in git curl jq ssh; do
    if ! have "$c"; then missing="$missing $c"; fi
  done
  missing=${missing# }
  if [ -z "$missing" ]; then res "уже есть: git, curl, jq, ssh"; return 0; fi
  mgr=
  if [ "$os" = macos ]; then
    if have brew; then mgr=brew; fi
  else
    if have pacman; then mgr=pacman; elif have apt-get; then mgr=apt; fi
  fi
  pkgs=
  for c in $missing; do
    case "$mgr:$c" in
      pacman:ssh) p=openssh ;;
      apt:ssh) p=openssh-client ;;
      *) p=$c ;;
    esac
    pkgs="$pkgs $p"
  done
  pkgs=${pkgs# }
  sudo_cmd=
  if [ "$os" = linux ] && [ "$(id -u)" -ne 0 ]; then sudo_cmd=sudo; fi
  noconfirm=
  if [ "$yes" -eq 1 ]; then noconfirm=--noconfirm; fi
  case "$mgr" in
    pacman) cmd="$sudo_cmd pacman -S --needed $noconfirm $pkgs" ;;
    apt) cmd="$sudo_cmd apt-get install -y $pkgs" ;;
    brew) cmd="brew install $pkgs" ;;
    *) cmd= ;;
  esac
  if [ "$dry" -eq 0 ]; then
    if [ -z "$mgr" ]; then
      stop "не хватает: $missing. Поставьте вручную (Linux: git, curl, jq, openssh; macOS: brew install git curl jq) и запустите снова"
    fi
    if [ "$yes" -eq 0 ]; then
      if ask_ok; then
        ask_yes_no "    Не хватает: $missing. Поставить через $mgr ($pkgs)? [Y/n]" y || stop "установка отменена"
      else
        stop "не хватает: $missing. Поставьте: $cmd — или запустите с --yes"
      fi
    fi
    if [ -n "$sudo_cmd" ] && ! have sudo; then stop "не хватает: $missing, а sudo нет: поставьте пакеты сами"; fi
  fi
  if [ -z "$cmd" ]; then
    res "не хватает: $missing; менеджера пакетов нет (dry-run)"
    return 0
  fi
  # shellcheck disable=SC2086
  run $cmd || fail "установка пакетов не удалась: $cmd"
  if [ "$dry" -eq 0 ]; then
    for c in $missing; do have "$c" || fail "после установки нет $c"; done
    res "поставлено: $pkgs"
  fi
}

# ---- step 3: herdr through mise ----

find_mise() {
  mise_bin=$(command -v mise 2>/dev/null || true)
  if [ -z "$mise_bin" ] && [ -x "$HOME/.local/bin/mise" ]; then mise_bin=$HOME/.local/bin/mise; fi
}
# mise where prints the install folder of the tool; a tool that is not installed has none.
mise_has_herdr() {
  find_mise
  [ -n "$mise_bin" ] || return 1
  p=$("$mise_bin" where github:herdrdev/herdr 2>/dev/null) || return 1
  [ -n "$p" ] && [ -d "$p" ]
}
path_block() { # file
  if [ -f "$1" ] && grep -qF -- "$path_begin" "$1"; then res "PATH для login shell уже в $1"; return 0; fi
  if [ "$dry" -eq 1 ]; then dry_say "добавить PATH-блок в $1"; return 0; fi
  {
    printf '\n%s\n' "$path_begin"
    # shellcheck disable=SC2016  # the line is written to the profile as is: $HOME must expand there
    printf '%s\n' 'export PATH="$HOME/.local/bin:$HOME/.local/share/mise/shims:$PATH"'
    printf '%s\n' "$path_end"
  } >>"$1"
  res "PATH для login shell записан в $1"
}
path_blocks() {
  path_block "$HOME/.profile"
  if [ -f "$HOME/.zprofile" ] || [ "$(basename "${SHELL:-}")" = zsh ]; then path_block "$HOME/.zprofile"; fi
}

step_herdr() {
  step 3 "herdr"
  if [ "$want_herdr" -eq 0 ]; then res "пропущено (--no-herdr)"; return 0; fi
  cur=$(herdr_version || true)
  managed=0
  if mise_has_herdr; then managed=1; fi
  if [ -n "$cur" ] && version_ge "$cur" "$herdr_min" && [ "$managed" -eq 0 ]; then
    res "уже стоит $cur (не через mise: не трогаю)"
    return 0
  fi
  find_mise
  if [ -z "$mise_bin" ]; then
    if [ "$dry" -eq 1 ]; then
      dry_say "curl -fsSL https://mise.run | sh"
    else
      have curl || fail "нет curl"
      curl -fsSL https://mise.run | sh || fail "не удалось поставить mise (https://mise.run)"
      find_mise
      [ -n "$mise_bin" ] || fail "mise не появился в ~/.local/bin"
    fi
  fi
  m=${mise_bin:-mise}
  if [ -n "$cur" ] && version_ge "$cur" "$herdr_min"; then
    if [ -n "${HERDR_VERSION:-}" ]; then
      run "$m" use -g "github:herdrdev/herdr@$HERDR_VERSION" || fail "mise use не удался"
    else
      run "$m" upgrade github:herdrdev/herdr || fail "mise upgrade не удался"
    fi
  else
    run "$m" use -g "github:herdrdev/herdr@${HERDR_VERSION:-latest}" || fail "mise use не удался"
  fi
  run "$m" reshim || true
  if [ "$dry" -eq 1 ]; then
    res "herdr будет установлен или обновлён через mise"
    path_blocks
    return 0
  fi
  new=$(herdr_version || true)
  if [ -z "$new" ] || ! version_ge "$new" "$herdr_min"; then
    fail "herdr после установки: '${new:-нет}', а нужен $herdr_min или новее"
  fi
  if [ -z "$cur" ]; then res "поставлен $new (mise)"
  elif [ "$new" = "$cur" ]; then res "уже новейший ($new)"
  else res "обновлён до $new"
  fi
  path_blocks
}

# ---- step 4: the herdr-projects plugin ----

# herdr asks before installing or updating a plugin, and refuses without --yes when stdin is not a
# terminal (as with `curl … | sh`). --yes on the installer also means yes here.
plugin_yes() { [ "$yes" -eq 1 ] || [ ! -t 0 ]; }
plugin_run() { # subcommand and its arguments
  if plugin_yes; then herdr plugin "$@" --yes; else herdr plugin "$@"; fi
}
plugin_run_dry() { # the line plugin_run would run
  if plugin_yes; then dry_say "herdr plugin $* --yes"; else dry_say "herdr plugin $*"; fi
}

step_plugin() {
  step 4 "плагин herdr-projects"
  if [ "$want_plugin" -eq 0 ]; then res "пропущено (--no-plugin)"; return 0; fi
  if [ "$dry" -eq 0 ] && ! have herdr; then res "herdr не найден: плагин пропущен"; return 0; fi
  before=$(hp_version || true)
  listed=0
  if have herdr && herdr plugin list 2>/dev/null | grep -q "$plugin_name"; then listed=1; fi
  newest=0
  if [ "$listed" -eq 1 ]; then
    if [ "$dry" -eq 1 ]; then
      plugin_run_dry update "$plugin_name"
    else
      # Only this plugin: an unnamed update would also update (and with --yes approve) every other GitHub
      # plugin, and its output is one line per plugin, which would hide whether this one is current.
      # herdr compares the remote commit itself: at the same one it prints
      # «herdr-projects is already up to date.» and reinstalls nothing, so there is nothing to skip here.
      # stdout is read for that line; stderr stays on the terminal, where herdr asks its question.
      out=$(plugin_run update "$plugin_name") || { say "$out"; fail "herdr plugin update не удался"; }
      say "$out"
      case "$out" in
        *"$plugin_name is already up to date"*) newest=1 ;;
      esac
    fi
  elif [ -n "${HERDR_PROJECTS_REF:-}" ]; then
    if [ "$dry" -eq 1 ]; then plugin_run_dry install "$plugin_repo" --ref "$HERDR_PROJECTS_REF"
    else plugin_run install "$plugin_repo" --ref "$HERDR_PROJECTS_REF" || fail "herdr plugin install не удался"
    fi
  else
    if [ "$dry" -eq 1 ]; then plugin_run_dry install "$plugin_repo"
    else plugin_run install "$plugin_repo" || fail "herdr plugin install не удался"
    fi
  fi
  hp=$HOME/.herdr-projects/data/bin/herdr-projects
  if [ "$dry" -eq 1 ]; then
    dry_say "ln -sf $hp $bin_dir/herdr-projects"
    return 0
  fi
  if [ -x "$hp" ]; then
    mkdir -p "$bin_dir"
    ln -sf "$hp" "$bin_dir/herdr-projects"
  fi
  after=$(hp_version || true)
  if [ -z "$after" ]; then res "herdr-projects не найден (проверьте: herdr plugin list)"; return 0; fi
  if [ "$newest" -eq 1 ]; then res "уже новейший: $after"
  elif [ -z "$before" ]; then res "поставлен: $after"
  elif [ "$after" = "$before" ]; then res "уже стоит: $after"
  else res "обновлён: $before → $after"
  fi
}

# ---- step 5: herdr-planner from the release archive (or --from-source) ----

download() { # url, file
  if code=$(curl -sSLf -o "$2" -w '%{http_code}' "$1" 2>"$work/curl.err"); then return 0; fi
  fail "не скачать $1 (HTTP ${code:-000}): $(head -n 1 "$work/curl.err" 2>/dev/null)"
}

fetch_release() {
  asset="herdr-planner-$os-$arch.tar.gz"
  if [ "$version" = latest ]; then base=$releases/latest/download; else base=$releases/download/$version; fi
  work=$(mktemp -d "${TMPDIR:-/tmp}/herdr-planner.XXXXXX")
  download "$base/$asset" "$work/$asset"
  download "$base/$asset.sha256" "$work/$asset.sha256"
  want=$(awk 'NR == 1 { print $1; exit }' "$work/$asset.sha256")
  got=$(sha256_of "$work/$asset")
  if [ "${#want}" -ne 64 ] || [ "$want" != "$got" ]; then
    fail "контрольная сумма $asset не совпала (в .sha256: ${want:-пусто}, у скачанного: $got): не ставлю"
  fi
  mkdir "$work/x"
  tar -xzf "$work/$asset" -C "$work/x" || fail "архив $asset не распаковался"
  for f in $archive_files; do
    [ -e "$work/x/$f" ] || fail "в архиве $asset нет $f"
  done
  ver=$(sed -n '1p' "$work/x/VERSION" | tr -d ' \r')
  [ -n "$ver" ] || fail "в архиве $asset пустой VERSION"
  if [ "$version" != latest ] && [ "v$ver" != "$version" ]; then
    fail "в архиве версия $ver, а запрошена $version"
  fi
  rel=$work/x
}

build_from_source() {
  [ -n "$src_root" ] || fail "--from-source нужен клон репозитория: запустите scripts/install.sh из него"
  rel=$src_root
  ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$src_root/Cargo.toml" | head -n 1)
  bin_src=$target_dir/release/herdr-planner
  if [ "$dry" -eq 1 ]; then
    dry_say "sh $src_root/scripts/web-build.sh"
    dry_say "cd $src_root && CARGO_TARGET_DIR=$target_dir cargo build --release"
    return 0
  fi
  [ -f "$src_root/scripts/web-build.sh" ] || fail "нет $src_root/scripts/web-build.sh"
  sh "$src_root/scripts/web-build.sh" || fail "сборка фронтенда не удалась"
  if cargo --version >/dev/null 2>&1; then
    (cd "$src_root" && CARGO_TARGET_DIR="$target_dir" cargo build --release) || fail "cargo build не удался"
  else
    have mise || fail "нет cargo и нет mise: поставьте Rust"
    (cd "$src_root" && CARGO_TARGET_DIR="$target_dir" mise x rust@stable -- cargo build --release) || fail "cargo build не удался"
  fi
  [ -x "$bin_src" ] || fail "после сборки нет $bin_src"
}

# $1 binary, $2 unpacked tree (its scripts/open.sh becomes herdr-planner-open)
install_bin() {
  if [ "$dry" -eq 1 ]; then
    dry_say "поставить $bin и $bin_dir/herdr-planner-open"
    planner_changed=1
    return 0
  fi
  mkdir -p "$bin_dir"
  if ! cmp -s "$1" "$bin"; then
    install -m 755 "$1" "$bin_dir/.herdr-planner.new"
    mv -f "$bin_dir/.herdr-planner.new" "$bin"
    planner_changed=1
  fi
  install -m 755 "$2/scripts/open.sh" "$bin_dir/herdr-planner-open"
}

step_planner() {
  step 5 "herdr-planner"
  if [ "$from_source" -eq 1 ]; then
    build_from_source
    install_bin "$bin_src" "$rel"
    if [ "$dry" -eq 0 ]; then res "собран из клона: $(bin_version) → $bin"; fi
    return 0
  fi
  fetch_release
  dest=$rel_root/$ver
  cur=
  if [ -x "$bin" ]; then cur=$(bin_version || true); fi
  if [ "$cur" = "$ver" ] && [ -d "$dest" ]; then
    rel=$dest
    res "уже стоит $ver: ничего не меняю"
    return 0
  fi
  if [ "$dry" -eq 1 ]; then
    dry_say "распаковать в $dest"
    if [ -n "$cur" ]; then res "будет обновлён: $cur → $ver"; else res "будет поставлен $ver"; fi
    install_bin "$rel/herdr-planner" "$rel"
    return 0
  fi
  mkdir -p "$rel_root"
  rm -rf "$dest.new"
  cp -R "$rel" "$dest.new"
  rm -rf "$dest"
  mv "$dest.new" "$dest"
  rel=$dest
  install_bin "$rel/herdr-planner" "$rel"
  got=$(bin_version || true)
  [ "$got" = "$ver" ] || fail "установленный бинарь показывает '${got:-ничего}', а ждали $ver"
  if [ -n "$cur" ]; then res "обновлён: $cur → $ver"; else res "поставлен $ver"; fi
}

# ---- step 6: service, window, Omarchy extras ----

wait_http() {
  u=$(probe_url)
  i=0
  while [ "$i" -lt 80 ] && ! curl -kfs --max-time 2 "$u/login" >/dev/null 2>&1; do
    sleep 0.25
    i=$((i + 1))
  done
  [ "$i" -lt 80 ] || fail "служба не отвечает на $u (логи: journalctl --user -u herdr-planner)"
}

service_path() {
  p=$bin_dir
  for tool in herdr-projects herdr claude; do
    t=$(command -v "$tool" 2>/dev/null || true)
    [ -n "$t" ] || continue
    d=$(dirname "$t")
    case ":$p:" in *":$d:"*) ;; *) p="$p:$d" ;; esac
  done
  printf '%s' "$p:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
}

service_state() {
  if [ "$os" = macos ]; then
    if ! have launchctl; then printf 'нет (launchctl не найден)'
    elif launchctl print "gui/$(id -u)/$label" >/dev/null 2>&1; then printf 'launchd: загружена'
    else printf 'launchd: не загружена'
    fi
    return 0
  fi
  if ! have systemctl; then printf 'нет (systemctl не найден)'; return 0; fi
  en=$(systemctl --user is-enabled "$unit" 2>/dev/null || true)
  ac=$(systemctl --user is-active "$unit" 2>/dev/null || true)
  printf 'systemd --user: %s, %s' "${en:-нет}" "${ac:-неактивна}"
}

service_systemd() {
  if ! have systemctl; then res "systemctl не найден: службу не ставлю, запускайте herdr-planner вручную"; return 0; fi
  if [ "$dry" -eq 1 ]; then dry_say "systemctl --user enable $unit и перезапуск при изменениях"; return 0; fi
  mkdir -p "$unit_dir"
  sed -e "s|@BIN@|$bin|" -e "s|@PATH@|$(service_path)|" "$rel/packaging/herdr-planner.service" >"$unit_dir/$unit.new"
  unit_changed=0
  if ! cmp -s "$unit_dir/$unit.new" "$unit_dir/$unit"; then unit_changed=1; fi
  mv -f "$unit_dir/$unit.new" "$unit_dir/$unit"
  systemctl --user daemon-reload >/dev/null 2>&1 || true
  systemctl --user enable "$unit" >/dev/null 2>&1 || true
  if [ "$planner_changed" -eq 0 ] && [ "$unit_changed" -eq 0 ] && systemctl --user is-active --quiet "$unit"; then
    res "служба работает: $unit на $(probe_url)"
    return 0
  fi
  systemctl --user restart "$unit" >/dev/null 2>&1 || fail "systemctl --user restart $unit не удался (journalctl --user -u herdr-planner)"
  wait_http
  res "служба: $unit на $(probe_url) (journalctl --user -u herdr-planner)"
}

service_launchd() {
  if ! have launchctl; then res "launchctl не найден: службу не ставлю"; return 0; fi
  if [ "$dry" -eq 1 ]; then dry_say "launchctl bootstrap gui/$(id -u) $plist (перезагрузка при изменениях)"; return 0; fi
  uid=$(id -u)
  mkdir -p "$(dirname "$plist")" "$HOME/Library/Logs"
  new=$(dirname "$plist")/.$label.plist.new
  sed -e "s|@BIN@|$bin|g" -e "s|@PATH@|$(service_path)|g" -e "s|@HOME@|$HOME|g" \
    "$rel/packaging/dev.herdr.planner.plist" >"$new"
  changed=0
  if [ ! -f "$plist" ] || ! cmp -s "$new" "$plist"; then changed=1; fi
  mv -f "$new" "$plist"
  loaded=0
  if launchctl print "gui/$uid/$label" >/dev/null 2>&1; then loaded=1; fi
  if [ "$loaded" -eq 1 ] && [ "$changed" -eq 0 ] && [ "$planner_changed" -eq 0 ]; then
    res "служба работает: launchd $label на $(probe_url)"
    return 0
  fi
  if [ "$loaded" -eq 1 ]; then
    launchctl bootout "gui/$uid/$label" >/dev/null 2>&1 || true
    sleep 1
  fi
  launchctl bootstrap "gui/$uid" "$plist" || fail "launchctl bootstrap не удался: $plist"
  wait_http
  res "служба: launchd $label на $(probe_url) (лог: ~/Library/Logs/herdr-planner.log)"
}

install_launcher() {
  apps=$data_home/applications
  icons=$data_home/icons/hicolor/scalable/apps
  if [ "$dry" -eq 1 ]; then dry_say "ярлык и значок в $apps"; return 0; fi
  mkdir -p "$apps" "$icons"
  sed "s|@OPEN@|$bin_dir/herdr-planner-open|" "$rel/packaging/herdr-planner.desktop" >"$apps/herdr-planner.desktop"
  install -m 644 "$rel/packaging/herdr-planner.svg" "$icons/herdr-planner.svg"
  if have update-desktop-database; then update-desktop-database "$apps" >/dev/null 2>&1 || true; fi
  res "ярлык: $apps/herdr-planner.desktop"
}

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
  # shellcheck disable=SC2046  # key_parts prints two words: the modifier mask and the key
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
  if [ -n "$hypr_key" ]; then printf '%s' "$hypr_key"; return 0; fi
  cur=$(current_key)
  if [ -n "$cur" ]; then printf '%s' "$cur"; return 0; fi
  for cand in "SUPER + ALT + P" "SUPER + SHIFT + H" "SUPER + SHIFT + L" "SUPER + SHIFT + T"; do
    if ! key_busy "$cand"; then printf '%s' "$cand"; return 0; fi
  done
  return 1
}

hypr_install() {
  if [ ! -f "$hypr_dir/bindings.lua" ]; then res "Hyprland: $hypr_dir/bindings.lua не найден: клавиша пропущена"; return 0; fi
  if [ "$dry" -eq 1 ]; then dry_say "клавиша Hyprland → $bin_dir/herdr-planner-open"; return 0; fi
  if ! chosen=$(pick_key); then
    res "Hyprland: нет свободной клавиши из кандидатов; укажите --key \"SUPER + ...\""
    return 0
  fi
  if [ -n "$hypr_key" ] && key_busy "$hypr_key"; then
    res "Hyprland: $hypr_key занята другой привязкой; выберите другую через --key"
    return 0
  fi
  cat >"$hypr_dir/herdr_planner.lua" <<EOF
-- Written by Herdr Planner's scripts/install.sh; the installer rewrites this file.
-- Opens the dashboard in its own app window.
o.bind("$chosen", "Herdr Planner", "$bin_dir/herdr-planner-open")
EOF
  if ! grep -qF -- "$hypr_begin" "$hypr_dir/bindings.lua"; then
    {
      printf '\n%s\n' "$hypr_begin"
      printf '%s\n' 'pcall(require, "hypr.herdr_planner")'
      printf '%s\n' "$hypr_end"
    } >>"$hypr_dir/bindings.lua"
  fi
  res "Hyprland: $chosen открывает Herdr Planner"
  if in_hyprland; then hyprctl reload >/dev/null 2>&1 || true; fi
}

hypr_uninstall() {
  if [ "$dry" -eq 1 ]; then dry_say "убрать ярлык Hyprland"; return 0; fi
  rm -f "$hypr_dir/herdr_planner.lua"
  if [ -f "$hypr_dir/bindings.lua" ] && grep -qF -- "$hypr_begin" "$hypr_dir/bindings.lua"; then
    tmp=$(mktemp "$hypr_dir/.bindings.lua.XXXXXX")
    awk -v b="$hypr_begin" -v e="$hypr_end" '
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

# ---- Omarchy bar icon ----

shell_edit() {
  filter=$1; shift
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

# shellcheck disable=SC2016  # jq programs: $id, $c, $i are jq variables
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
# shellcheck disable=SC2016  # jq programs: $id, $c, $i are jq variables
filter_remove='
  def mine: type == "object" and .id == $id;
  (if (.bar.layout | type) == "object" then
     .bar.layout |= with_entries(.value |= (if type == "array" then map(select(mine | not)) else . end))
   else . end)
  | (if (.plugins | type) == "array" then .plugins |= map(select(mine | not)) else . end)
  | (if (.disabledPlugins | type) == "array" then .disabledPlugins |= map(select(. != $id)) else . end)'

shell_install() {
  if [ ! -f "$shell_json" ]; then res "значок в панели: $shell_json не найден"; return 0; fi
  if ! have jq; then res "значок в панели: нет jq"; return 0; fi
  if [ "$(jq -r '.bar.layout.center | type' "$shell_json" 2>/dev/null)" != array ]; then
    res "значок в панели: в $shell_json нет раскладки бара"
    return 0
  fi
  if [ "$dry" -eq 1 ]; then dry_say "значок в панели в $plugin_dir и $shell_json"; return 0; fi
  mkdir -p "$plugin_dir"
  rm -f "$plugin_dir"/*.qml "$plugin_dir"/manifest.json
  install -m 644 "$rel/packaging/omarchy-plugin/manifest.json" "$rel/packaging/omarchy-plugin/Widget.qml" "$plugin_dir/"
  p=$(port)
  if [ "$p" = 7117 ]; then p=0; fi
  r=$(shell_edit "$filter_add" --argjson port "$p") || { res "не удалось изменить $shell_json"; return 0; }
  case "$r" in
    same) r="уже на месте" ;;
    *) r="добавлен в раскладку бара" ;;
  esac
  res "значок в панели: $r ($plugin_dir)"
  if [ "$shell_reload" -eq 1 ]; then omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; fi
}

shell_uninstall() {
  if [ "$dry" -eq 1 ]; then dry_say "убрать значок из панели Omarchy"; return 0; fi
  if [ -f "$shell_json" ] && have jq; then shell_edit "$filter_remove" >/dev/null || true; fi
  if [ -f "$plugin_dir/manifest.json" ] && grep -q "\"id\": \"$plugin_id\"" "$plugin_dir/manifest.json"; then
    rm -rf "$plugin_dir"
  fi
  if [ "$shell_reload" -eq 1 ]; then omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; fi
}

step_service() {
  step 6 "служба и окно"
  if [ -d "$omarchy_dir" ]; then omarchy=1; fi
  if [ "$want_service" -eq 0 ]; then
    res "служба не трогается (--no-service)"
  elif [ "$os" = macos ]; then
    service_launchd
  else
    service_systemd
  fi
  if [ "$os" = linux ] && [ "$omarchy" -eq 1 ]; then
    install_launcher
    if [ "$hypr" -eq 1 ]; then hypr_install; else res "Hyprland: пропущено (--no-hypr)"; fi
    if [ "$shell" -eq 1 ]; then shell_install; else res "значок в панели: пропущено (--no-shell)"; fi
  else
    res "ярлык, клавиша Hyprland и значок в панели: пропущено: не Omarchy"
  fi
}

# ---- step 7: login and address ----

# Stores the login with set-auth. $1 user, $2 plain password (empty keeps the stored hash), $3 listen (empty keeps it).
apply_login() {
  if [ "$dry" -eq 1 ]; then
    dry_say "записать в $cfg: логин «$1»${3:+, адрес $3} и хеш пароля"
    return 0
  fi
  [ -x "$bin" ] || fail "сначала установите: sh install.sh"
  if [ -n "$2" ]; then
    hash=$(printf '%s' "$2" | "$bin" hash-password) || fail "не удалось хешировать пароль"
  else
    hash=$(cfg_str password_hash)
  fi
  [ -n "$hash" ] || stop "для входа нужен пароль: --password-stdin или --password-file"
  if [ -n "$3" ]; then
    printf '%s' "$hash" | "$bin" set-auth --user "$1" --listen "$3" >/dev/null || fail "set-auth не удался"
  else
    printf '%s' "$hash" | "$bin" set-auth --user "$1" >/dev/null || fail "set-auth не удался"
  fi
  res "вход: «$1», адрес $(listen_addr):$(port)"
  restart_service
}

read_password_source() { # into pw
  case "$pw_source" in
    stdin) IFS= read -r pw || pw= ;;
    file)
      [ -r "$pw_file" ] || stop "не читается $pw_file"
      IFS= read -r pw <"$pw_file" || pw=
      ;;
  esac
  [ -n "$pw" ] || stop "пустой пароль"
}

read_password_tty() { # into pw, asked twice
  n=0
  while :; do
    printf '    Пароль: '
    read_secret
    pw1=$pw
    printf '    Ещё раз: '
    read_secret
    pw2=$pw
    if [ -z "$pw1" ]; then say "    пустой пароль, ещё раз"
    elif [ "$pw1" != "$pw2" ]; then say "    пароли не совпали, ещё раз"
    else pw=$pw1; return 0
    fi
    n=$((n + 1))
    [ "$n" -lt 3 ] || stop "пароль не задан"
  done
}

restart_service() {
  if [ "$dry" -eq 1 ]; then return 0; fi
  if [ "$os" = macos ]; then
    if [ -f "$plist" ]; then launchctl kickstart -k "gui/$(id -u)/$label" >/dev/null 2>&1 || true; fi
  elif have systemctl && systemctl --user is-enabled --quiet "$unit" 2>/dev/null; then
    systemctl --user restart "$unit" >/dev/null 2>&1 || true
  fi
}

login_interactive() {
  say "    Доступ с других машин: браузер на другом компьютере или телефоне."
  if ! ask_yes_no "    Открыть доступ с других машин? [y/N]" n; then
    res "оставляю как есть: $(probe_url)"
    return 0
  fi
  printf '    Логин [herdr]: '
  read_tty
  user=${ans:-herdr}
  read_password_tty
  printf '    Адрес для входа [0.0.0.0]: '
  read_tty
  addr=${ans:-0.0.0.0}
  apply_login "$user" "$pw" "$addr"
}

step_login() {
  step 7 "вход и адрес"
  if [ -n "$pw_source" ]; then
    read_password_source
    apply_login "$(login_name)" "$pw" "$listen"
  elif [ -n "$login_user" ] || [ -n "$listen" ]; then
    apply_login "$(login_name)" "" "$listen"
  elif ask_ok; then
    login_interactive
  elif has_cfg password_hash; then
    res "вход: как было ($(probe_url))"
  else
    res "вход без пароля, только эта машина: $(probe_url)"
    res "с других машин: sh install.sh password --listen 0.0.0.0 (или запустите установку в терминале)"
  fi
}

# ---- summary ----

summary() {
  say ""
  say "Итог:"
  hv=$(herdr_version || true)
  pv=$(hp_version || true)
  bv=
  if [ -x "$bin" ]; then bv=$(bin_version || true); fi
  say "  herdr            ${hv:-нет}"
  say "  herdr-projects   ${pv:-нет}"
  say "  herdr-planner    ${bv:-нет}"
  say "  служба           $(service_state)"
  hn=$(host_name)
  if is_loopback; then
    say "  адрес            $(probe_url) (только эта машина)"
  else
    say "  адрес            $(probe_url); с других машин: $(scheme)://$hn:$(port) (логин: $(login_name))"
    if ! has_cfg tls_cert; then
      say "                   браузеру на другой машине нужен HTTPS: sh install.sh tls --cert FILE --key FILE"
    fi
  fi
  say ""
  if [ "$dry" -eq 0 ] && [ -n "$pv" ]; then
    chk=$("${SHELL:-/bin/sh}" -lc 'herdr-projects version' 2>/dev/null | head -n 1 || true)
    if [ -n "$chk" ]; then
      say "  проверено: login shell видит herdr-projects ($chk)"
    else
      say "  login shell не видит herdr-projects: проверьте PATH в ~/.profile"
    fi
  fi
}

# ---- actions ----

do_install() {
  if [ "$dry" -eq 1 ]; then say "Пробный прогон (--dry-run): ничего не меняю, только печатаю шаги."; fi
  step_platform
  step_packages
  step_herdr
  step_plugin
  step_planner
  step_service
  step_login
  summary
}

do_uninstall() {
  if [ "$os" = macos ]; then
    if [ "$dry" -eq 1 ]; then
      dry_say "launchctl bootout gui/$(id -u)/$label; rm $plist"
    else
      if have launchctl; then launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true; fi
      rm -f "$plist"
    fi
  elif [ "$want_service" -eq 1 ] && have systemctl; then
    if [ "$dry" -eq 1 ]; then
      dry_say "systemctl --user disable --now $unit; rm $unit_dir/$unit"
    else
      systemctl --user disable --now "$unit" >/dev/null 2>&1 || true
      rm -f "$unit_dir/$unit"
      systemctl --user daemon-reload >/dev/null 2>&1 || true
    fi
  fi
  if [ "$hypr" -eq 1 ]; then hypr_uninstall; fi
  if [ "$shell" -eq 1 ]; then shell_uninstall; fi
  run rm -f "$bin" "$bin_dir/herdr-planner-open" "$bin_dir/.herdr-planner.new" \
    "$data_home/applications/herdr-planner.desktop" \
    "$data_home/icons/hicolor/scalable/apps/herdr-planner.svg"
  run rm -rf "$rel_root"
  if [ "$dry" -eq 0 ] && have update-desktop-database; then
    update-desktop-database "$data_home/applications" >/dev/null 2>&1 || true
  fi
  say "Удалено: планировщик, служба, ярлык, клавиша, значок в панели, распакованные релизы."
  say "Осталось: $cfg и $state_home/herdr-planner (журнал, чат). herdr и плагин herdr-projects не тронуты."
}

do_status() {
  say "Herdr Planner ($os $arch)"
  for f in "$bin" "$bin_dir/herdr-planner-open"; do
    if [ -e "$f" ]; then say "ok       $f"; else say "missing  $f"; fi
  done
  if [ "$os" = macos ]; then f=$plist; else f=$unit_dir/$unit; fi
  if [ -e "$f" ]; then say "ok       $f"; else say "missing  $f"; fi
  if [ -f "$cfg" ]; then
    say "config   $cfg"
    if has_cfg password_hash; then say "login    on ($(login_name) / $(listen_addr):$(port))"; else say "login    off (127.0.0.1 only)"; fi
    if has_cfg tls_cert; then say "tls      on"; else say "tls      off"; fi
  else
    say "config   $cfg (не записан: вход выключен, 127.0.0.1)"
  fi
  say "service  $(service_state)"
  if [ -f "$shell_json" ] && have jq; then
    if jq -e --arg id "$plugin_id" 'any(.bar.layout[]?; type == "array" and any(.[]; type == "object" and .id == $id))' "$shell_json" >/dev/null 2>&1; then
      say "bar      $plugin_id is in the bar layout"
    else
      say "bar      $plugin_id is not in the bar layout"
    fi
  fi
  u=$(probe_url)
  if curl -kfs --max-time 2 "$u/login" >/dev/null 2>&1; then say "http     answers on $u"; else say "http     no answer on $u"; fi
  summary
}

do_password() {
  if [ "$dry" -eq 0 ] && [ ! -x "$bin" ]; then fail "сначала установите: sh install.sh"; fi
  say "Пароль хранится только как Argon2id-хеш в $cfg."
  if [ -n "$pw_source" ]; then
    read_password_source
  elif ask_ok; then
    read_password_tty
  else
    stop "нужен пароль: --password-stdin, --password-file или запуск в терминале"
  fi
  apply_login "$(login_name)" "$pw" "$listen"
}

do_tls() {
  if [ -z "$cert" ] || [ -z "$key_arg" ]; then stop "tls: нужны --cert FILE --key FILE"; fi
  [ -r "$cert" ] || fail "нет $cert"
  [ -r "$key_arg" ] || fail "нет $key_arg"
  if [ "$dry" -eq 1 ]; then dry_say "записать TLS-пути в $cfg"; return 0; fi
  [ -x "$bin" ] || fail "сначала установите: sh install.sh"
  "$bin" set-tls --cert "$cert" --key "$key_arg" || fail "set-tls не удался"
  restart_service
}

main() {
  work=
  trap cleanup EXIT
  trap 'exit 1' INT TERM
  parse_args "$@"
  setup_env
  case "$action" in
    install) do_install ;;
    uninstall) do_uninstall ;;
    status) do_status ;;
    password) do_password ;;
    tls) do_tls ;;
  esac
}

main "$@"
