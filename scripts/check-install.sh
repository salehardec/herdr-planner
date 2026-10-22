#!/bin/sh
# Checks scripts/install.sh and the one-line install.sh without touching the real home.
# HOME and every XDG folder live in a temporary folder. The tools the installer calls
# are stand-ins in a PATH folder that only record what they were asked: mise, herdr,
# systemctl, launchctl, pacman, brew, curl (web addresses), uname (macOS). The release is
# built here from the planner binary given as $1, plus a wrapper that reports the next
# version, and served from file://. Runs with setsid, so there is no terminal to ask on.
#   scripts/check-install.sh [path-to-binary]
set -eu
cd "$(dirname "$0")/.."
REPO=$(pwd)
BIN_ARG=${1:-$HOME/.cache/herdr-planner/target/release/herdr-planner}
BIN=$(cd "$(dirname "$BIN_ARG")" && pwd)/$(basename "$BIN_ARG")
[ -x "$BIN" ] || { echo "check-install: no binary at $BIN" >&2; exit 1; }
command -v jq >/dev/null 2>&1 || { echo "check-install: jq is needed" >&2; exit 1; }
command -v setsid >/dev/null 2>&1 || { echo "check-install: setsid is needed (no-terminal runs)" >&2; exit 1; }
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT INT TERM
ST=$T/state; FB=$T/fakebin; EX=$T/extra; AB=$T/allbin; H=$T/home
mkdir -p "$ST" "$FB" "$EX" "$AB" "$T/tmp" "$T/rel" "$H/.config/hypr" "$H/.config/omarchy"
REAL_CURL=$(command -v curl)
REAL_UNAME=$(command -v uname)
PATHX=$FB:$EX:$AB
LAST=$T/last.log

fail() { echo "check-install: $*" >&2; [ ! -s "$LAST" ] || { echo "--- last output:" >&2; cat "$LAST" >&2; }; exit 1; }
ok() { echo "ok   $*"; }
check() { # description, then a command that must succeed
  desc=$1; shift
  if "$@"; then ok "$desc"; else fail "failed: $desc"; fi
}

# ---- stand-ins ----

stub() { # name; body on stdin
  { printf '#!/bin/sh\nST=%s\nEX=%s\nREAL_CURL=%s\nREAL_UNAME=%s\n' "$ST" "$EX" "$REAL_CURL" "$REAL_UNAME"; cat; } >"$FB/$1"
  chmod +x "$FB/$1"
}
stub systemctl <<'EOF'
echo "$*" >>"$ST/systemctl.log"
case "$*" in
  *is-active*)
    if [ -e "$ST/active" ]; then q=0; else q=3; fi
    case "$*" in *--quiet*) ;; *) if [ "$q" -eq 0 ]; then echo active; else echo inactive; fi ;; esac
    exit "$q" ;;
  *is-enabled*)
    if [ -e "$ST/enabled" ]; then echo enabled; exit 0; fi
    echo disabled; exit 1 ;;
  *disable*) rm -f "$ST/enabled" "$ST/active" ;;
  *enable*) : >"$ST/enabled" ;;
  *restart* | *start*) : >"$ST/active" ;;
  *stop*) rm -f "$ST/active" ;;
esac
exit 0
EOF
stub curl <<'EOF'
for a in "$@"; do case "$a" in file://*) exec "$REAL_CURL" "$@" ;; esac; done
echo "$*" >>"$ST/curl.log"
exit 0
EOF
stub sleep <<'EOF'
exit 0
EOF
stub pgrep <<'EOF'
exit 1
EOF
stub sudo <<'EOF'
echo "sudo $*" >>"$ST/sudo.log"
exec "$@"
EOF
stub pacman <<'EOF'
echo "$*" >>"$ST/pacman.log"
for c in git ssh; do printf '#!/bin/sh\nexit 0\n' >"$EX/$c"; chmod +x "$EX/$c"; done
exit 0
EOF
stub brew <<'EOF'
echo "$*" >>"$ST/brew.log"
for c in git ssh; do printf '#!/bin/sh\nexit 0\n' >"$EX/$c"; chmod +x "$EX/$c"; done
exit 0
EOF
stub mise <<'EOF'
echo "$*" >>"$ST/mise.log"
case "$*" in
  --version) echo "2026.10.1" ;;
  "where github:herdrdev/herdr") [ -e "$ST/managed" ] || exit 1; echo "$ST" ;;
  "use -g github:herdrdev/herdr@latest") echo 0.9.4 >"$ST/herdr-ver"; : >"$ST/managed" ;;
  "use -g github:herdrdev/herdr@"*) echo "${*##*@}" >"$ST/herdr-ver"; : >"$ST/managed" ;;
  "upgrade github:herdrdev/herdr") echo 0.9.4 >"$ST/herdr-ver"; : >"$ST/managed" ;;
  reshim) ;;
  *) echo "mise stand-in: unexpected $*" >&2; exit 1 ;;
esac
exit 0
EOF
stub herdr <<'EOF'
echo "$*" >>"$ST/herdr.log"
plugin_bin="$HOME/.herdr-projects/data/bin/herdr-projects"
write_plugin() {
  mkdir -p "$HOME/.herdr-projects/data/bin"
  printf '#!/bin/sh\necho "herdr-projects %s"\n' "$1" >"$plugin_bin"
  chmod +x "$plugin_bin"
  printf '%s\n' "$1" >"$ST/plugin-ver"
}
# herdr refuses install and update without --yes when stdin is not a terminal.
plugin_yes() {
  case " $* " in *" --yes "* | *" -y "*) return 0 ;; esac
  echo "$1 requires --yes when stdin is not interactive" >&2
  exit 2
}
case "$1" in
  --version) echo "herdr $(cat "$ST/herdr-ver")" ;;
  plugin)
    case "$2" in
      list) [ -e "$ST/plugin" ] && echo "herdr-projects $(cat "$ST/plugin-ver" 2>/dev/null || echo 1.4.2) enabled" ;;
      install)
        plugin_yes "remote plugin install" "$@"
        : >"$ST/plugin"
        write_plugin "$(cat "$ST/plugin-latest" 2>/dev/null || echo 1.4.2)" ;;
      update)
        plugin_yes "plugin update" "$@"
        scope=
        for a in "$@"; do
          case "$a" in update | --yes | -y) ;; *) scope=$a ;; esac
        done
        if [ -n "$scope" ] && [ "$scope" != "herdr-projects" ]; then
          echo "plugin not installed: $scope" >&2
          exit 1
        fi
        # Another GitHub plugin: an unnamed update touches it too (the real herdr does).
        if [ -z "$scope" ] && [ -e "$ST/other-plugin" ]; then
          echo "augie-git is already up to date."
          echo augie-git >"$ST/other.log"
        fi
        [ -e "$ST/plugin" ] || exit 0
        cur=$(cat "$ST/plugin-ver" 2>/dev/null || true)
        latest=$(cat "$ST/plugin-latest" 2>/dev/null || echo 1.4.2)
        if [ "$cur" = "$latest" ]; then
          echo "herdr-projects is already up to date."
          exit 0
        fi
        write_plugin "$latest"
        echo "Updated herdr-projects from github:eliasstravik/herdr-projects." ;;
      *) echo "herdr stand-in: unexpected plugin $*" >&2; exit 1 ;;
    esac ;;
  *) echo "herdr stand-in: unexpected $*" >&2; exit 1 ;;
esac
exit 0
EOF
stub launchctl <<'EOF'
echo "$*" >>"$ST/launchctl.log"
case "$1" in
  print) [ -e "$ST/launchd" ] && exit 0; exit 113 ;;
  bootstrap) : >"$ST/launchd" ;;
  bootout) rm -f "$ST/launchd" ;;
esac
exit 0
EOF
stub uname <<'EOF'
if [ -e "$ST/darwin" ]; then
  case "$1" in
    -s) echo Darwin; exit 0 ;;
    -m) echo arm64; exit 0 ;;
  esac
fi
exec "$REAL_UNAME" "$@"
EOF
for c in git ssh; do printf '#!/bin/sh\nexit 0\n' >"$EX/$c"; chmod +x "$EX/$c"; done
# Every system tool except git and ssh (those are stand-ins in $EX and can be removed).
for d in /usr/bin /bin; do
  for f in "$d"/*; do
    n=${f##*/}
    case "$n" in git | ssh | scp | sftp | ssh-*) continue ;; esac
    if [ ! -e "$AB/$n" ] && [ ! -L "$AB/$n" ]; then ln -s "$f" "$AB/$n"; fi
  done
done

# Omarchy as it is on the machine: a bar with the clock, a Hyprland bindings file.
printf -- '-- mine\no.bind("X", "x", "y")\n' >"$H/.config/hypr/bindings.lua"
cat >"$T/shell.in" <<'EOF2'
{"bar":{"centerAnchor":"my.clock","layout":{"center":[{"id":"omarchy.indicators"},{"id":"my.clock","chip":false},{"id":"other.pulse"}],"left":[{"id":"a.menu"}],"right":["b.tray"]},"position":"top"},"plugins":[{"id":"lock.explorer","design":"x"}],"version":1}
EOF2
jq . "$T/shell.in" >"$H/.config/omarchy/shell.json"
cp "$H/.config/omarchy/shell.json" "$T/shell.orig"
cp "$H/.config/hypr/bindings.lua" "$T/bindings.orig"
echo 0.8.0 >"$ST/herdr-ver"
printf 'cert\n' >"$T/cert.pem"
printf 'key\n' >"$T/key.pem"

# ---- running the installer ----

# Every run starts from an empty environment with the test folders, and without a terminal.
sh_env() { # script, args: output to $LAST; the status is the script's
  setsid -w env -i HOME="$H" PATH="$PATHX" SHELL=/bin/sh TMPDIR="$T/tmp" LC_ALL=C.UTF-8 \
    XDG_CONFIG_HOME="$H/.config" XDG_DATA_HOME="$H/.local/share" XDG_CACHE_HOME="$H/.cache" \
    XDG_STATE_HOME="$H/.local/state" HERDR_PLANNER_RELEASES="file://$T/rel" \
    HERDR_PLANNER_TARGET_DIR="$T/target" \
    /bin/sh "$@" >"$LAST" 2>&1 </dev/null
}
inst() { sh_env scripts/install.sh "$@"; }
cfg_get() { awk -F'"' -v k="$1" '$0 ~ ("^" k " = ") { print $2; exit }' "$CFG"; }

snap() { # every file, link and folder under $H with a content hash
  (cd "$H" && find . \( -type f -o -type l \) | LC_ALL=C sort | while IFS= read -r f; do
    printf '%s %s\n' "$(sha256sum "$f" | awk '{print $1}')" "$f"
  done; find . -type d | LC_ALL=C sort)
}
count() { [ -e "$1" ] && wc -l <"$1" || echo 0; }
has() { grep -q -- "$2" "$1"; }

publish() { # version, binary, [latest]
  v=$1; b=$2
  d=$T/pub-$v
  rm -rf "$d"
  mkdir -p "$d/scripts" "$d/packaging/omarchy-plugin"
  cp "$b" "$d/herdr-planner"
  chmod 755 "$d/herdr-planner"
  cp scripts/install.sh scripts/open.sh "$d/scripts/"
  cp packaging/herdr-planner.service packaging/herdr-planner.desktop packaging/herdr-planner.svg \
    packaging/dev.herdr.planner.plist "$d/packaging/"
  cp packaging/omarchy-plugin/manifest.json packaging/omarchy-plugin/Widget.qml "$d/packaging/omarchy-plugin/"
  cp README.md "$d/"
  printf '%s\n' "$v" >"$d/VERSION"
  out=$T/rel/download/v$v
  mkdir -p "$out"
  for plat in linux-x86_64 macos-aarch64; do
    (cd "$d" && COPYFILE_DISABLE=1 tar -czf "$out/herdr-planner-$plat.tar.gz" .)
    (cd "$out" && sha256sum "herdr-planner-$plat.tar.gz" >"herdr-planner-$plat.tar.gz.sha256")
  done
  cp scripts/install.sh "$out/install.sh"
  if [ "${3:-}" = latest ]; then
    rm -rf "$T/rel/latest"
    mkdir -p "$T/rel/latest"
    cp -R "$out" "$T/rel/latest/download"
  fi
}
publish_bad_sum() { # version: a release whose .sha256 does not match its archive
  publish "$1" "$BIN"
  printf '%s  herdr-planner-linux-x86_64.tar.gz\n' 0000000000000000000000000000000000000000000000000000000000000000 \
    >"$T/rel/download/v$1/herdr-planner-linux-x86_64.tar.gz.sha256"
}

# ---- the checks ----

for f in install.sh scripts/install.sh scripts/check-install.sh scripts/run-checks.sh; do
  sh -n "$f" || fail "sh -n $f"
done
if command -v bash >/dev/null 2>&1; then bash --posix -n scripts/install.sh || fail "bash --posix -n scripts/install.sh"; fi
if command -v shellcheck >/dev/null 2>&1 && shellcheck --version >/dev/null 2>&1; then shellcheck scripts/install.sh install.sh scripts/check-install.sh || fail "shellcheck"; fi
ok "shell syntax"
check "launchd plist is well-formed XML" python3 -I -c 'import sys, xml.dom.minidom as m; m.parse(sys.argv[1])' packaging/dev.herdr.planner.plist

V1=$("$BIN" --version | awk '{print $2}')
# The next release: the same binary behind a wrapper that answers --version with 0.4.1.
V2=0.4.1
mkdir -p "$T/wrap"
# shellcheck disable=SC2016  # the wrapper's own $1/$@ must stay literal
printf '#!/bin/sh\ncase "${1:-}" in --version | -V) echo "herdr-planner %s"; exit 0 ;; esac\nexec %s "$@"\n' "$V2" "$BIN" >"$T/wrap/herdr-planner"
chmod +x "$T/wrap/herdr-planner"
publish "$V1" "$BIN"
publish "$V2" "$T/wrap/herdr-planner" latest
CFG=$H/.config/herdr-planner/config.toml
PLIST=$H/Library/LaunchAgents/dev.herdr.planner.plist

# 1. dry-run: every step printed, nothing changed.
before=$(snap)
printf 'correct horse\n' | setsid -w env -i HOME="$H" PATH="$PATHX" SHELL=/bin/sh TMPDIR="$T/tmp" LC_ALL=C.UTF-8 \
  XDG_CONFIG_HOME="$H/.config" XDG_DATA_HOME="$H/.local/share" XDG_CACHE_HOME="$H/.cache" \
  XDG_STATE_HOME="$H/.local/state" HERDR_PLANNER_RELEASES="file://$T/rel" \
  /bin/sh scripts/install.sh install --dry-run --yes --user me --password-stdin --version "v$V1" >"$LAST" 2>&1 \
  || fail "dry-run exit status"
[ "$(grep -c '^\[[1-7]/7\] ' "$LAST")" -eq 7 ] || fail "dry-run must print steps 1 to 7"
has "$LAST" '(dry-run)' || fail "dry-run must mark what it would do"
[ "$before" = "$(snap)" ] || fail "dry-run changed the home folder"
[ ! -e "$ST/pacman.log" ] || fail "dry-run must not run a package manager"
if [ -e "$ST/mise.log" ] && grep -q 'use -g' "$ST/mise.log"; then fail "dry-run must not install herdr"; fi
ok "--dry-run prints the seven steps and changes nothing"
[ -z "${CHECK_INSTALL_VERBOSE:-}" ] || cat "$LAST"

# 2. the first install: planner from the release, herdr through mise, service, Omarchy bits.
inst install --yes --version "v$V1" || fail "first install"
check "first install: the binary is the release's" sh -c "[ \"\$('$H/.local/bin/herdr-planner' --version)\" = 'herdr-planner $V1' ]"
check "first install: the release tree is unpacked" test -f "$H/.local/share/herdr-planner/release/$V1/packaging/omarchy-plugin/Widget.qml"
check "first install: open.sh comes from the archive" cmp -s "$H/.local/share/herdr-planner/release/$V1/scripts/open.sh" "$H/.local/bin/herdr-planner-open"
check "first install: the widget is installed from the archive" cmp -s "$H/.local/share/herdr-planner/release/$V1/packaging/omarchy-plugin/Widget.qml" "$H/.config/omarchy/plugins/salehardec.herdr-planner/Widget.qml"
check "first install: herdr via mise (fresh install)" grep -q 'use -g github:herdrdev/herdr@latest' "$ST/mise.log"
check "first install: herdr-projects symlinked" test -L "$H/.local/bin/herdr-projects"
check "first install: the plugin install says --yes without a terminal" grep -q 'plugin install .*--yes' "$ST/herdr.log"
check "first install: unit enabled and restarted" sh -c "grep -q 'enable herdr-planner.service' '$ST/systemctl.log' && grep -q 'restart herdr-planner.service' '$ST/systemctl.log'"
check "first install: the unit runs the installed binary" grep -q "ExecStart=$H/.local/bin/herdr-planner" "$H/.config/systemd/user/herdr-planner.service"
check "first install: one PATH block in ~/.profile" test "$(grep -c '>>> herdr-planner PATH' "$H/.profile")" -eq 1
check "first install: bar entry right after the clock" sh -c "[ \"\$(jq -c '[.bar.layout.center[].id]' '$H/.config/omarchy/shell.json')\" = '[\"omarchy.indicators\",\"my.clock\",\"salehardec.herdr-planner\",\"other.pulse\"]' ]"
check "first install: Hyprland key written" grep -q '"SUPER + ALT + P", "Herdr Planner"' "$H/.config/hypr/herdr_planner.lua"
check "first install: launcher written" grep -q "^Exec=$H/.local/bin/herdr-planner-open" "$H/.local/share/applications/herdr-planner.desktop"
check "first install: the no-password note" has "$LAST" 'вход без пароля'
check "first install: no config without a password" test ! -e "$CFG"
check "first install: no far galaxy hint" sh -c "! grep -q 'herdr machine add' '$LAST'"
check "first install: the summary has the planner version" has "$LAST" "herdr-planner    $V1"
check "first install: seven steps" test "$(grep -c '^\[[1-7]/7\] ' "$LAST")" -eq 7
[ -z "${CHECK_INSTALL_VERBOSE:-}" ] || cat "$LAST"
ok "first install: planner, herdr, plugin, service, Omarchy bits, summary"

# 3. the same version again: already installed, the tree unchanged, the service not restarted.
: >"$ST/other-plugin"   # a second GitHub plugin, also current
before=$(snap)
rs_before=$(grep -c -- '--user restart' "$ST/systemctl.log" || true)
inst install --yes --version "v$V1" || fail "second install"
has "$LAST" "уже стоит $V1" || fail "same version must say it is already installed"
has "$LAST" 'уже новейший' || fail "an up-to-date plugin must say it is already the newest"
check "same version: another plugin is not updated" test ! -e "$ST/other.log"
[ "$before" = "$(snap)" ] || fail "same version changed the home folder"
[ "$(grep -c -- '--user restart' "$ST/systemctl.log" || true)" -eq "$rs_before" ] || fail "same version restarted the service"
check "same version: PATH block not repeated" test "$(grep -c '>>> herdr-planner PATH' "$H/.profile")" -eq 1
check "same version: one shell.json backup only" test "$(find "$H/.config/omarchy" -maxdepth 1 -name 'shell.json.bak-herdr-planner-*' | wc -l)" -eq 1
ok "same version: already installed, tree unchanged, no restart"

# 3b. the plugin is behind: herdr updates it, still without a terminal.
printf '1.4.3\n' >"$ST/plugin-latest"
inst install --yes --version "v$V1" || fail "plugin update"
has "$LAST" 'обновлён: 1.4.2 → 1.4.3' || fail "plugin update: message"
check "plugin update: names herdr-projects and passes --yes without a terminal" grep -q 'plugin update herdr-projects --yes' "$ST/herdr.log"
check "plugin update: another plugin is not updated" test ! -e "$ST/other.log"
check "plugin update: the plugin is at 1.4.3" sh -c "[ \"\$('$H/.local/bin/herdr-projects' version)\" = 'herdr-projects 1.4.3' ]"
ok "plugin update: behind, updated with --yes"

# 4. a new version: updated, service restarted, the old release kept.
rs_before=$(grep -c -- '--user restart' "$ST/systemctl.log" || true)
inst install --yes || fail "update to $V2"
has "$LAST" "обновлён: $V1 → $V2" || fail "update must say what changed"
check "update: the binary is $V2" sh -c "[ \"\$('$H/.local/bin/herdr-planner' --version)\" = 'herdr-planner $V2' ]"
check "update: the new release tree is unpacked" test -d "$H/.local/share/herdr-planner/release/$V2"
check "update: the old release tree is kept" test -d "$H/.local/share/herdr-planner/release/$V1"
check "update: service restarted" sh -c "[ \"\$(grep -c -- '--user restart' '$ST/systemctl.log')\" -gt $rs_before ]"
ok "new version: updated, service restarted"

# 5. a wrong checksum: refused, nothing changed.
publish_bad_sum 0.4.2
before=$(snap)
if inst install --yes --version v0.4.2; then fail "a wrong checksum must refuse"; fi
has "$LAST" 'контрольная сумма' || fail "wrong checksum: message"
[ "$before" = "$(snap)" ] || fail "a refused archive changed the home folder"
ok "wrong checksum: refused, nothing changed"

# 6. a download that fails: the address, the HTTP code and the archive name are named.
if inst install --yes --version v9.9.9 --no-herdr --no-plugin --no-service; then fail "a missing release must fail"; fi
has "$LAST" 'не скачать file://' || fail "download failure must name the address"
has "$LAST" 'HTTP 000' || fail "download failure must name the HTTP code"
has "$LAST" 'herdr-planner-linux-x86_64.tar.gz' || fail "download failure must name the archive"
ok "download failure: address, code and archive named"

# 7. missing packages: nothing runs without --yes and a terminal; with --yes sudo installs them.
rm -f "$EX/git" "$EX/ssh" "$ST/pacman.log"
if inst install --no-herdr --no-plugin --no-service; then fail "missing packages without --yes must stop"; fi
has "$LAST" 'не хватает: git ssh' || fail "missing packages must be listed"
[ ! -e "$ST/pacman.log" ] || fail "no package manager may run without --yes"
inst install --yes --no-herdr --no-plugin --no-service || fail "--yes must install the missing packages"
has "$ST/pacman.log" 'git openssh' || fail "pacman must get git and openssh"
has "$ST/sudo.log" 'sudo pacman' || fail "packages must go through sudo"
ok "missing packages: listed without --yes, installed with --yes through sudo"

# 8. the login: a password from stdin, then flags that change only part of it.
printf 'correct horse\n' | setsid -w env -i HOME="$H" PATH="$PATHX" SHELL=/bin/sh TMPDIR="$T/tmp" LC_ALL=C.UTF-8 \
  XDG_CONFIG_HOME="$H/.config" XDG_DATA_HOME="$H/.local/share" XDG_CACHE_HOME="$H/.cache" \
  XDG_STATE_HOME="$H/.local/state" \
  /bin/sh scripts/install.sh password --password-stdin --user me --listen 0.0.0.0 >"$LAST" 2>&1 || fail "password --password-stdin"
check "password: config has the user" [ "$(cfg_get user)" = me ]
check "password: config has the listen address" [ "$(cfg_get listen)" = 0.0.0.0 ]
# shellcheck disable=SC2016  # a regex: \$ is a literal dollar
check "password: a hash is stored" grep -q '^password_hash = "\$argon2id\$' "$CFG"
check "password: the password itself is not stored" sh -c "! grep -q 'correct horse' '$CFG'"
check "password: config is private (600)" [ "$(stat -c %a "$CFG")" = 600 ]
inst status || fail "status after password"
check "status: login on, with the address" has "$LAST" 'login    on (me / 0.0.0.0:7117)'
check "status: no far galaxy hint" sh -c "! grep -q 'herdr machine add' '$LAST'"
hash_before=$(cfg_get password_hash)
inst install --no-herdr --no-plugin --no-service --user other --listen 127.0.0.1 || fail "--user --listen"
check "flags --user --listen: user and listen changed" [ "$(cfg_get user)" = other ]
check "flags --user --listen: listen changed" [ "$(cfg_get listen)" = 127.0.0.1 ]
check "flags --user --listen: the hash is kept" [ "$(cfg_get password_hash)" = "$hash_before" ]
printf 'second pass\n' >"$T/pw.txt"
inst install --no-herdr --no-plugin --no-service --user me --password-file "$T/pw.txt" --listen 0.0.0.0 || fail "--password-file"
check "flags --password-file: new hash" [ "$(cfg_get password_hash)" != "$hash_before" ]
check "flags --password-file: listen and user set" [ "$(cfg_get listen)" = 0.0.0.0 ]
rm -f "$CFG"
if inst install --no-herdr --no-plugin --no-service --listen 0.0.0.0; then fail "a remote address without a password must refuse"; fi
has "$LAST" 'нужен пароль' || fail "remote address without password: message"
check "a refused login writes no config" test ! -e "$CFG"
inst install --no-herdr --no-plugin --no-service || fail "install without a terminal"
has "$LAST" 'вход без пароля' || fail "no terminal, no flags: say there is no password"
check "no terminal, no flags: no config" test ! -e "$CFG"
ok "login: flags write a hash, no terminal means no password"

# 8b. questions on a terminal: a pseudo-terminal answers them (missing packages, then the login).
rm -f "$EX/git" "$EX/ssh" "$ST/pacman.log" "$CFG"
python3 -I - "$T" "$PATHX" "$H" "$REPO" tty-packages <<'PY' || fail "terminal: packages question"
import os, pty, select, sys, time
T, PATHX, H, REPO, mode = sys.argv[1:6]
env = {"HOME": H, "PATH": PATHX, "SHELL": "/bin/sh", "TMPDIR": T + "/tmp", "LC_ALL": "C.UTF-8",
       "XDG_CONFIG_HOME": H + "/.config", "XDG_DATA_HOME": H + "/.local/share",
       "XDG_CACHE_HOME": H + "/.cache", "XDG_STATE_HOME": H + "/.local/state",
       "HERDR_PLANNER_RELEASES": "file://" + T + "/rel", "TERM": "dumb"}
if mode == "tty-packages":
    args = ["install", "--no-herdr", "--no-plugin", "--no-service"]
    answers = [("Поставить через pacman", "y\n"), ("Открыть доступ с других машин", "n\n")]
else:
    args = ["install", "--no-herdr", "--no-plugin", "--no-service"]
    answers = [("Открыть доступ с других машин", "y\n"), ("Логин [herdr]", "me\n"),
               ("Пароль:", "pw-one\n"), ("Ещё раз:", "pw-one\n"), ("Адрес для входа [0.0.0.0]", "\n")]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(REPO)
    os.execve("/bin/sh", ["/bin/sh", "scripts/install.sh"] + args, env)
buf = b""
deadline = time.time() + 90
while answers and time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            data = os.read(fd, 4096)
        except OSError:
            break
        if not data:
            break
        buf += data
    if answers[0][0].encode() in buf:
        os.write(fd, answers[0][1].encode())
        answers.pop(0)
        buf = b""
status = 1
while time.time() < deadline:
    done, st = os.waitpid(pid, os.WNOHANG)
    if done:
        status = os.waitstatus_to_exitcode(st) if hasattr(os, "waitstatus_to_exitcode") else os.WEXITSTATUS(st)
        break
    try:
        os.read(fd, 4096)
    except OSError:
        pass
    time.sleep(0.1)
sys.exit(0 if not answers and status == 0 else 1)
PY
check "terminal: the packages question was asked and answered" grep -q 'git openssh' "$ST/pacman.log"
rm -f "$ST/pacman.log"
python3 -I - "$T" "$PATHX" "$H" "$REPO" tty-login <<'PY' || fail "terminal: login questions"
import os, pty, select, sys, time
T, PATHX, H, REPO, mode = sys.argv[1:6]
env = {"HOME": H, "PATH": PATHX, "SHELL": "/bin/sh", "TMPDIR": T + "/tmp", "LC_ALL": "C.UTF-8",
       "XDG_CONFIG_HOME": H + "/.config", "XDG_DATA_HOME": H + "/.local/share",
       "XDG_CACHE_HOME": H + "/.cache", "XDG_STATE_HOME": H + "/.local/state",
       "HERDR_PLANNER_RELEASES": "file://" + T + "/rel", "TERM": "dumb"}
args = ["install", "--no-herdr", "--no-plugin", "--no-service"]
answers = [("Открыть доступ с других машин", "y\n"), ("Логин [herdr]", "me\n"),
           ("Пароль:", "pw-one\n"), ("Ещё раз:", "pw-one\n"), ("Адрес для входа [0.0.0.0]", "\n")]
pid, fd = pty.fork()
if pid == 0:
    os.chdir(REPO)
    os.execve("/bin/sh", ["/bin/sh", "scripts/install.sh"] + args, env)
buf = b""
deadline = time.time() + 90
while answers and time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            data = os.read(fd, 4096)
        except OSError:
            break
        if not data:
            break
        buf += data
    if answers[0][0].encode() in buf:
        os.write(fd, answers[0][1].encode())
        answers.pop(0)
        buf = b""
status = 1
while time.time() < deadline:
    done, st = os.waitpid(pid, os.WNOHANG)
    if done:
        status = os.WEXITSTATUS(st)
        break
    try:
        os.read(fd, 4096)
    except OSError:
        pass
    time.sleep(0.1)
sys.exit(0 if not answers and status == 0 else 1)
PY
check "terminal: login asked, password typed twice, address defaulted" [ "$(cfg_get user)" = me ]
check "terminal: address is 0.0.0.0 by default" [ "$(cfg_get listen)" = 0.0.0.0 ]
# shellcheck disable=SC2016  # a regex: \$ is a literal dollar
check "terminal: the password is hashed" grep -q '^password_hash = "\$argon2id\$' "$CFG"

# 9. TLS paths land in the same config.
inst tls --cert "$T/cert.pem" --key "$T/key.pem" || fail "tls"
check "tls: certificate path" [ "$(cfg_get tls_cert)" = "$T/cert.pem" ]
check "tls: key path" [ "$(cfg_get tls_key)" = "$T/key.pem" ]
ok "tls: paths written"

# 10. --from-source dry-run: the build steps are printed, nothing runs.
before=$(snap)
inst install --dry-run --from-source --no-herdr --no-plugin --no-service || fail "--from-source --dry-run"
has "$LAST" 'web-build.sh' || fail "--from-source must print the web build"
has "$LAST" 'cargo build --release' || fail "--from-source must print the cargo build"
[ "$before" = "$(snap)" ] || fail "--from-source --dry-run changed the home folder"
ok "--from-source --dry-run prints the build and changes nothing"

# 11. uninstall (Linux): the planner goes, herdr and the plugin stay, Omarchy files are given back.
inst uninstall || fail "uninstall"
check "uninstall: binary, unit and release trees removed" sh -c "[ ! -e '$H/.local/bin/herdr-planner' ] && [ ! -e '$H/.config/systemd/user/herdr-planner.service' ] && [ ! -e '$H/.local/share/herdr-planner/release' ]"
check "uninstall: Hyprland bindings back byte for byte" cmp -s "$T/bindings.orig" "$H/.config/hypr/bindings.lua"
check "uninstall: shell.json back as it was" sh -c "[ \"\$(jq -S -c . '$H/.config/omarchy/shell.json')\" = \"\$(jq -S -c . '$T/shell.orig')\" ]"
check "uninstall: herdr-projects left in place" test -e "$H/.herdr-projects/data/bin/herdr-projects"
check "uninstall: config kept" test -f "$CFG"
ok "uninstall: planner removed, the rest kept"

# 12. one line: the root install.sh fetches the release installer; piped as curl does it, or as a file.
mkdir -p "$T/oneliner"
cp install.sh "$T/oneliner/install.sh"
# shellcheck disable=SC2002  # a pipe on purpose: this is how `curl … | sh` feeds the script
cat install.sh | setsid -w env -i HOME="$H" PATH="$PATHX" SHELL=/bin/sh TMPDIR="$T/tmp" LC_ALL=C.UTF-8 \
  XDG_CONFIG_HOME="$H/.config" XDG_DATA_HOME="$H/.local/share" XDG_CACHE_HOME="$H/.cache" \
  XDG_STATE_HOME="$H/.local/state" HERDR_PLANNER_RELEASES="file://$T/rel" \
  /bin/sh -s -- --yes --no-herdr --no-plugin --no-service >"$LAST" 2>&1 || fail "one line, piped"
check "one line (piped): the latest release is installed" sh -c "[ \"\$('$H/.local/bin/herdr-planner' --version)\" = 'herdr-planner $V2' ]"
sh_env "$T/oneliner/install.sh" --help || fail "install.sh as a file, no clone"
has "$LAST" 'Herdr Planner: установка' || fail "one line as a file: help text"
check "from a clone, install.sh runs scripts/install.sh" sh -c "cd '$REPO' && sh install.sh --help | grep -q 'Herdr Planner: установка'"
ok "one line: piped and as a file both reach the installer"

# 13. Omarchy absent: the launcher, the key and the bar are skipped, with a reason.
mv "$H/.config/omarchy" "$T/omarchy.away"
inst install --no-herdr --no-plugin --version "v$V1" || fail "install without Omarchy"
has "$LAST" 'пропущено: не Omarchy' || fail "without Omarchy: say what is skipped"
mv "$T/omarchy.away" "$H/.config/omarchy"
ok "no Omarchy: the Omarchy-only steps are skipped with a reason"

# 14. macOS (uname stand-in): the LaunchAgent is written and bootstrapped; systemd is not called.
: >"$ST/darwin"
sys_before=$(count "$ST/systemctl.log")
inst install --yes --no-herdr --no-plugin || fail "macOS install"
check "macOS: LaunchAgent written with the binary path" grep -q "<string>$H/.local/bin/herdr-planner</string>" "$PLIST"
check "macOS: no placeholder left in the plist" sh -c "! grep -q '@BIN@\|@PATH@\|@HOME@' '$PLIST'"
check "macOS: logs folder exists" test -d "$H/Library/Logs"
check "macOS: bootstrapped into the gui domain" grep -q 'bootstrap gui/' "$ST/launchctl.log"
check "macOS: systemd not called" [ "$(count "$ST/systemctl.log")" = "$sys_before" ]
boots=$(grep -c bootstrap "$ST/launchctl.log" || true)
inst install --yes --no-herdr --no-plugin || fail "macOS second run"
has "$LAST" 'служба работает: launchd' || fail "macOS second run: the service already runs"
check "macOS: second run does not bootstrap again" [ "$(grep -c bootstrap "$ST/launchctl.log")" = "$boots" ]
inst uninstall || fail "macOS uninstall"
check "macOS: uninstall boots out and removes the LaunchAgent" sh -c "grep -q 'bootout gui/' '$ST/launchctl.log' && [ ! -e '$PLIST' ]"
rm -f "$ST/darwin"
ok "macOS: LaunchAgent install, quiet second run, uninstall"

# 15. the release archive itself: layout as released.
tar -tzf "$T/rel/download/v$V1/herdr-planner-linux-x86_64.tar.gz" >"$T/listing"
for f in herdr-planner scripts/install.sh scripts/open.sh packaging/omarchy-plugin/Widget.qml \
  packaging/omarchy-plugin/manifest.json packaging/herdr-planner.service packaging/herdr-planner.desktop \
  packaging/herdr-planner.svg packaging/dev.herdr.planner.plist README.md VERSION; do
  grep -q "^\(\./\)\{0,1\}$f\$" "$T/listing" || fail "archive has no $f"
done
ok "archive layout: binary, scripts, packaging, widget, plist, README, VERSION"

echo "check-install: ok"
