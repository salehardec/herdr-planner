#!/bin/sh
# Everything that can be checked offline: Rust tests, the installer and launcher
# checks, then the page against a fixture board in headless Chromium.
#   scripts/run-checks.sh [screenshot-dir]
# The build lives outside the repository (NTFS): ~/.cache/herdr-planner/target.
set -eu
cd "$(dirname "$0")/.."
TARGET="${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}"
export CARGO_TARGET_DIR="$TARGET"
CARGO="${CARGO:-cargo}"
$CARGO --version >/dev/null 2>&1 || CARGO="mise x rust@stable -- cargo"
SHOTS="${1:-}"
[ -z "$SHOTS" ] || mkdir -p "$SHOTS"
$CARGO test --quiet
$CARGO build --quiet
sh scripts/check-install.sh "$TARGET/debug/herdr-planner"
sh scripts/check-open.sh
FX="$(mktemp -d)"
SERVER=
AUTH_SERVER=
trap '[ -z "$SERVER" ] || kill "$SERVER" 2>/dev/null || true; [ -z "$AUTH_SERVER" ] || kill "$AUTH_SERVER" 2>/dev/null || true; rm -rf "$FX"' EXIT INT TERM
node scripts/fixture.mjs "$FX" >/dev/null
mkdir -p "$FX/cfg/herdr-planner" "$FX/state"
# A stale server on a PID-derived port would answer instead of ours: ask the
# kernel for a free one (Node is already required by the page checks).
free_port() {
  node -e 'const net=require("net");const start=+process.argv[1];let p=start;const tryOne=()=>{const s=net.createServer();s.once("error",()=>{s.close();p++;if(p<start+300)tryOne();else process.exit(1);});s.once("listening",()=>s.close(()=>console.log(p)));s.listen(p,"127.0.0.1");};tryOne();' "$1"
}
PORT=$(free_port 7600)
FIXTURE_DIR="$FX" \
  HERDR_PROJECTS_BIN="$FX/bin/herdr-projects" \
  HERDR_SOCKET_PATH="$FX/herdr.sock" \
  OMARCHY_THEME_DIR="$FX/theme" \
  PATH="$FX/bin:$PATH" \
  XDG_STATE_HOME="$FX/state" XDG_CONFIG_HOME="$FX/cfg" XDG_CACHE_HOME="$FX/cache" \
  "$TARGET/debug/herdr-planner" --port "$PORT" --data "$FX/board" >"$FX/server.log" 2>&1 &
SERVER=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  curl -fs "http://127.0.0.1:$PORT/api/state" >/dev/null 2>&1 && break
  sleep 0.25
done
FIXTURE="$FX" HERDR_PLANNER_TARGET_DIR="$TARGET" node scripts/ui-check.mjs "http://127.0.0.1:$PORT" ${SHOTS:+"$SHOTS"}

# A second server with a password: the login, the cookie and the limit.
AUTH_PORT=$(free_port $((PORT + 1)))
mkdir -p "$FX/cfg-auth"
HASH=$(printf 'secret\n' | "$TARGET/debug/herdr-planner" hash-password)
printf '%s' "$HASH" | XDG_CONFIG_HOME="$FX/cfg-auth" "$TARGET/debug/herdr-planner" set-auth --user me --listen 127.0.0.1 >/dev/null
FIXTURE_DIR="$FX" \
  HERDR_PROJECTS_BIN="$FX/bin/herdr-projects" \
  HERDR_SOCKET_PATH="$FX/herdr.sock" \
  OMARCHY_THEME_DIR="$FX/theme" \
  PATH="$FX/bin:$PATH" \
  XDG_STATE_HOME="$FX/state-auth" XDG_CONFIG_HOME="$FX/cfg-auth" XDG_CACHE_HOME="$FX/cache" \
  "$TARGET/debug/herdr-planner" --port "$AUTH_PORT" --data "$FX/board" >"$FX/server-auth.log" 2>&1 &
AUTH_SERVER=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  curl -fs "http://127.0.0.1:$AUTH_PORT/login" >/dev/null 2>&1 && break
  sleep 0.25
done
AUTH_BASE="http://127.0.0.1:$AUTH_PORT" node scripts/auth-check.mjs
