#!/bin/sh
# Everything that can be checked offline, in one go: unit tests, then the page
# against a fixture world (fake herdr-projects / herdr / claude, real git repos).
#   scripts/run-checks.sh [screenshot-dir]
# Needs cargo (mise x rust@stable -- if it is not on PATH), node >= 22 and chromium.
set -eu
cd "$(dirname "$0")/.."
CARGO="${CARGO:-cargo}"
$CARGO --version >/dev/null 2>&1 || CARGO="mise x rust@stable -- cargo"
SHOTS="${1:-}"
[ -z "$SHOTS" ] || mkdir -p "$SHOTS"
$CARGO test --quiet
$CARGO build --quiet
sh scripts/check-install.sh ./target/debug/herdr-planner
sh scripts/check-open.sh
FX="$(mktemp -d)"
STATE="$(mktemp -d)"
SERVER=
trap '[ -z "$SERVER" ] || kill "$SERVER" 2>/dev/null || true; rm -rf "$FX" "$STATE"' EXIT INT TERM
node scripts/fixture.mjs "$FX" >/dev/null
# The group suggestion starts switched off: the checks turn it on where they test it.
mkdir -p "$STATE/herdr-planner"
printf '{"settings":{"autogroup":"off"}}' >"$STATE/herdr-planner/state.json"
PORT=$((7300 + $$ % 500))
FIXTURE_DIR="$FX" HERDR_PROJECTS_BIN="$FX/bin/herdr-projects" HERDR_BIN="$FX/bin/herdr" \
  PATH="$FX/bin:$PATH" CLAUDE_CONFIG_DIR="$FX/claude" XDG_STATE_HOME="$STATE" HERDR_PLANNER_OPEN_WAIT_SECS=10 HERDR_PLANNER_FRESH_DEBOUNCE_SECS=2 \
  ./target/debug/herdr-planner --port "$PORT" --root "$FX/root" >"$FX/server.log" 2>&1 &
SERVER=$!
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  curl -fs "http://127.0.0.1:$PORT/api/snapshot" >/dev/null 2>&1 && break
  sleep 0.25
done
FIXTURE="$FX" node scripts/ui-check.mjs "http://127.0.0.1:$PORT" ${SHOTS:+"$SHOTS"}
