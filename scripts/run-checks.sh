#!/bin/sh
# Everything that can be checked offline: the page's unit tests and build, Rust tests,
# the installer and launcher checks, then the page against a fixture board in headless
# Chromium (the Galaxy), a big fixture (50 universes / 500 planets) and the login.
#   scripts/run-checks.sh [screenshot-dir]
# The builds live outside the repository (NTFS): ~/.cache/herdr-planner/{target,web}.
set -eu
cd "$(dirname "$0")/.."
TARGET="${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}"
export CARGO_TARGET_DIR="$TARGET"
CARGO="${CARGO:-cargo}"
$CARGO --version >/dev/null 2>&1 || CARGO="mise x rust@stable -- cargo"
SHOTS="${1:-}"
[ -z "$SHOTS" ] || mkdir -p "$SHOTS"
sh scripts/web-build.sh --test
sh scripts/web-build.sh
$CARGO test --quiet
$CARGO build --quiet
sh scripts/check-install.sh "$TARGET/debug/herdr-planner"
sh scripts/check-open.sh
# The page checks run side by side, each against its own fixture board, its own server and its own
# headless browser: the Galaxy in four shards (the sections do not depend on each other), a big world
# (50 universes / 500 planets), and the login. Their output is kept apart and printed in order.
FX="$(mktemp -d)"
PIDS=
trap '[ -z "$PIDS" ] || kill $PIDS 2>/dev/null || true; rm -rf "$FX" "$FX"-*' EXIT INT TERM
# A stale server on a PID-derived port would answer instead of ours: ask the
# kernel for a free one (Node is already required by the page checks).
free_port() {
  node -e 'const net=require("net");const start=+process.argv[1];let p=start;const tryOne=()=>{const s=net.createServer();s.once("error",()=>{s.close();p++;if(p<start+300)tryOne();else process.exit(1);});s.once("listening",()=>s.close(()=>console.log(p)));s.listen(p,"127.0.0.1");};tryOne();' "$1"
}

# start_server NAME [--scale AxB]: a fixture under $FX-NAME and the planner on it; sets D (dir) and P (port).
start_server() {
  D="$FX-$1"; shift
  node scripts/fixture.mjs "$D" "$@" >/dev/null
  mkdir -p "$D/cfg/herdr-planner" "$D/state"
  READY_PATH=/api/state
  STATE_NAME=state
  if [ -n "${PASSWORD:-}" ]; then
    # A server with a password (user me, listening on 127.0.0.1 only).
    HASH=$(printf '%s\n' "$PASSWORD" | "$TARGET/debug/herdr-planner" hash-password)
    printf '%s' "$HASH" | XDG_CONFIG_HOME="$D/cfg" "$TARGET/debug/herdr-planner" set-auth --user me --listen 127.0.0.1 >/dev/null
    READY_PATH=/login
    STATE_NAME=state-auth   # where auth-check looks for the chat attachments
  fi
  P=$(free_port "$NEXT_PORT"); NEXT_PORT=$((P + 1))
  # The stand-ins of the other programs: ssh (the other machines), claude (translation), gh/glab, codex sessions.
  env HERDR_PLANNER_HOST=orion CODEX_HOME="$D/codex" HERDR_PLANNER_SSH="$D/bin/ssh" HERDR_PLANNER_CLAUDE="$D/bin/claude" HERDR_PLANNER_GH="$D/bin/gh" \
    HERDR_PLANNER_GLAB="$D/bin/glab" HERDR_BIN="$D/bin/herdr" HERDR_PLANNER_TERMINAL="$D/bin/xdg-terminal-exec" \
    FIXTURE_DIR="$D" HERDR_PROJECTS_BIN="$D/bin/herdr-projects" HERDR_SOCKET_PATH="$D/herdr.sock" \
    OMARCHY_THEME_DIR="$D/theme" CLAUDE_CONFIG_DIR="$D/claude" PATH="$D/bin:$PATH" \
    XDG_STATE_HOME="$D/$STATE_NAME" XDG_CONFIG_HOME="$D/cfg" XDG_CACHE_HOME="$D/cache" \
    "$TARGET/debug/herdr-planner" --port "$P" --data "$D/board" >"$D/server.log" 2>&1 &
  PIDS="$PIDS $!"
  READY=
  for _ in $(seq 1 60); do
    curl -fs "http://127.0.0.1:$P$READY_PATH" >/dev/null 2>&1 && READY=1 && break
    sleep 0.25
  done
  [ -n "$READY" ] || { echo "run-checks: the dashboard did not start on port $P"; cat "$D/server.log"; exit 1; }
}

# A check that hangs must fail, not hold the whole run (a stuck browser, a lost connection): at most 12 minutes each.
limited() {
  if command -v timeout >/dev/null 2>&1; then timeout -k 10 720 "$@"; else "$@"; fi
}

NEXT_PORT=7600
JOBS=
OUT="$FX-out"
mkdir -p "$OUT"

# The Galaxy: sections split in four, each with a fresh board (a section that needs another's state is a bug).
n=0
for only in "boot,live,navigator,autopilot,manual,stack,queue,search" \
            "chat,todo,focus,requests,links,archive,limits" \
            "far,lag,galaxies,remote" \
            "station,crew,arrival,ship,theme,simple,narrow"; do
  n=$((n + 1))
  start_server "ui$n"
  FIXTURE="$D" HERDR_PLANNER_TARGET_DIR="$TARGET" ONLY="$only" limited node scripts/ui-check.mjs "http://127.0.0.1:$P" ${SHOTS:+"$SHOTS"} >"$OUT/ui$n.txt" 2>&1 &
  JOBS="$JOBS $!:ui$n"
done

# A big world: 50 universes / 500 planets must stay orderly and cheap (the scale fixture).
start_server big --scale 50x500
FIXTURE="$D" limited node scripts/perf-check.mjs "http://127.0.0.1:$P" ${SHOTS:+"$SHOTS"} >"$OUT/big.txt" 2>&1 &
JOBS="$JOBS $!:big"

# A server with a password: the login, the cookie and the limit; the ship's computer behind it.
PASSWORD=secret start_server auth
AUTH_D="$D"; AUTH_P="$P"
(
  # before auth-check's limit test: a wrong old password is refused in the server's words
  SHIP_LOGIN="me:secret" ONLY="ship login" FIXTURE="$AUTH_D" limited node scripts/ui-check.mjs "http://127.0.0.1:$AUTH_P" ${SHOTS:+"$SHOTS"} &&
    FIXTURE="$AUTH_D" AUTH_BASE="http://127.0.0.1:$AUTH_P" limited node scripts/auth-check.mjs
) >"$OUT/auth.txt" 2>&1 &
JOBS="$JOBS $!:auth"

FAILED=
for j in $JOBS; do
  pid="${j%%:*}"; name="${j#*:}"
  if wait "$pid"; then :; else FAILED="$FAILED $name"; fi
  echo "══ $name"
  cat "$OUT/$name.txt"
done
if [ -n "$FAILED" ]; then echo "run-checks: failed:$FAILED"; exit 1; fi
echo "run-checks: ok"
