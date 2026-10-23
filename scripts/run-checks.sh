#!/bin/sh
# Everything that can be checked offline: the page's unit tests and build, Rust tests,
# the installer and launcher checks, then the page against a fixture board in headless
# Chromium (the Galaxy), a big fixture (50 universes / 500 planets) and the login.
#   scripts/run-checks.sh [screenshot-dir]
#   PAGE_ONLY=1 scripts/run-checks.sh   only the page checks, on the binary and the page already built
#   SHARDS="ui2 big" scripts/run-checks.sh   only these parts of the page checks (ui1 … ui9, big, auth; default: all; ui6–ui9 are the layout at every scale);
#                                      CI runs each part on a runner of its own
#   SECTIONS="runners todo" scripts/run-checks.sh   one screen: one fixture, one server, one browser, only these sections
#                                      of the Galaxy (no builds: the page and the binary are taken as they are). Scripts/yard.sh
#                                      section uses it to look at one screen while working on it.
# When HERDR_PROJECTS_PROGRESS is set (the board's file, see docs/check-progress.md), every finished part reports «k/N parts».
# The builds live outside the repository (NTFS): ~/.cache/herdr-planner/{target,web}.
set -eu
cd "$(dirname "$0")/.."
# One screen is a look at the page, never a build: the same rule as PAGE_ONLY for the whole Galaxy.
[ -z "${SECTIONS:-}" ] || PAGE_ONLY=1
TARGET="${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}"
export CARGO_TARGET_DIR="$TARGET"
CARGO="${CARGO:-cargo}"
$CARGO --version >/dev/null 2>&1 || CARGO="mise x rust@stable -- cargo"
SHOTS="${1:-}"
[ -z "$SHOTS" ] || mkdir -p "$SHOTS"
if [ -z "${PAGE_ONLY:-}" ]; then
  sh scripts/web-build.sh --test
  sh scripts/web-build.sh --check
  sh scripts/web-build.sh
  $CARGO test --quiet
  $CARGO build --quiet
  sh scripts/check-install.sh "$TARGET/debug/herdr-planner"
  sh scripts/check-open.sh
fi
# The page checks run side by side, each against its own fixture board, its own server and its own
# headless browser: the Galaxy in nine shards (the sections do not depend on each other), a big world
# (50 universes / 500 planets), and the login. Their output is kept apart and printed in order.
FX="$(mktemp -d)"
PIDS=
trap '[ -z "$PIDS" ] || kill $PIDS 2>/dev/null || true; rm -rf "$FX" "$FX"-* "$FX.jobs"' EXIT INT TERM
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
  # The port is free when asked for, not necessarily when the planner binds it (another run of these checks on the
  # same machine may take it in between): the planner exits at once when it cannot bind, and a server that answers on the
  # port must be OUR process, or the checks would talk to somebody else's board. If it is not, the next port.
  READY=
  for _attempt in 1 2 3 4 5 6; do
    P=$(free_port "$NEXT_PORT"); NEXT_PORT=$((P + 1))
    # The stand-ins of the other programs: claude (translation), gh/glab, codex sessions.
    env HERDR_PLANNER_HOST=orion HERDR_PLANNER_LOAD_FILE="$D/load/load.json" HERDR_PLANNER_LOAD_MS=400 HERDR_PLANNER_PROC_FILE="$D/load/procs.json" HERDR_PLANNER_WORKTREES="$D/worktrees" HERDR_PLANNER_STRAY_MS=500 HERDR_PLANNER_STRAY_FIRST_MS=200 HERDR_PLANNER_STRAY_GRACE_MS=1500 HERDR_PLANNER_STRAY_TERM_MS=300 CODEX_HOME="$D/codex" HERDR_PLANNER_SSH="$D/bin/ssh" HERDR_PLANNER_CLAUDE="$D/bin/claude" HERDR_PLANNER_GH="$D/bin/gh" \
      HERDR_PLANNER_GLAB="$D/bin/glab" HERDR_BIN="$D/bin/herdr" HERDR_PLANNER_TERMINAL="$D/bin/xdg-terminal-exec" \
      HERDR_PLANNER_OWN_TERMS=bob-the-builder-host,host:app-semantics,host:salehost FIXTURE_DIR="$D" HERDR_PROJECTS_BIN="$D/bin/herdr-projects" HERDR_SOCKET_PATH="$D/herdr.sock" \
      OMARCHY_THEME_DIR="$D/theme" CLAUDE_CONFIG_DIR="$D/claude" PATH="$D/bin:$PATH" \
      XDG_STATE_HOME="$D/$STATE_NAME" XDG_CONFIG_HOME="$D/cfg" XDG_CACHE_HOME="$D/cache" \
      "$TARGET/debug/herdr-planner" --port "$P" --data "$D/board" >"$D/server.log" 2>&1 &
    SPID=$!
    # up to a minute: on a loaded machine the planner starts slowly
    for _ in $(seq 1 240); do
      kill -0 "$SPID" 2>/dev/null || break
      if curl -fs "http://127.0.0.1:$P$READY_PATH" >/dev/null 2>&1; then READY=1; break; fi
      sleep 0.25
    done
    if [ -n "$READY" ] && kill -0 "$SPID" 2>/dev/null && owns_port "$SPID" "$P"; then break; fi
    kill "$SPID" 2>/dev/null || true
    wait "$SPID" 2>/dev/null || true
    READY=
  done
  [ -n "$READY" ] || { echo "run-checks: the dashboard did not start on port $P"; cat "$D/server.log"; exit 1; }
  PIDS="$PIDS $SPID"
}

# Is the process that listens on the port ours? (Without `ss` there is nothing to ask: believed.)
owns_port() {
  command -v ss >/dev/null 2>&1 || return 0
  ss -ltnpH "sport = :$2" 2>/dev/null | grep -q "pid=$1," 
}

# A check that hangs must fail, not hold the whole run (a stuck browser, a lost connection): at most 12 minutes each.
limited() {
  if command -v timeout >/dev/null 2>&1; then timeout -k 10 720 "$@"; else "$@"; fi
}

# Is this part of the page checks wanted? (SHARDS empty: all of them)
want() { [ -z "${SHARDS:-}" ] || case " $SHARDS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

NEXT_PORT="${NEXT_PORT:-$((7600 + $$ % 1000))}"   # spread, so that two runs on one machine seldom meet on a port
JOBS=
OUT="$FX-out"
mkdir -p "$OUT"
# Six headless Chromium instances at once on a four-core runner make the page checks miss their own
# waits (the checks are about behaviour, not throughput). At most three run at a time; each keeps its
# own browser, server and fixture.
# (A machine with more cores takes more of them at once: half the cores, between three and six. RUN_CHECKS_JOBS sets it.)
CORES="$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
DEFAULT_JOBS=$((CORES / 2)); [ "$DEFAULT_JOBS" -ge 3 ] || DEFAULT_JOBS=3; [ "$DEFAULT_JOBS" -le 6 ] || DEFAULT_JOBS=6
MAX_JOBS=${RUN_CHECKS_JOBS:-$DEFAULT_JOBS}
JOBQ="$FX.jobs"
mkfifo "$JOBQ"
exec 9<>"$JOBQ"
i=0
while [ "$i" -lt "$MAX_JOBS" ]; do printf 'x\n' >&9; i=$((i + 1)); done
job_slot() { read -r _ <&9 || true; }
job_release() { printf 'x\n' >&9; }

# The parts this run has to do: the page checks split in nine, the big world and the login (or the one screen of SECTIONS).
PARTS_TOTAL=0
if [ -n "${SECTIONS:-}" ]; then
  PARTS_TOTAL=1
else
  for n in 1 2 3 4 5 6 7 8 9; do if want "ui$n"; then PARTS_TOTAL=$((PARTS_TOTAL + 1)); fi; done
  if want big; then PARTS_TOTAL=$((PARTS_TOTAL + 1)); fi
  if want auth; then PARTS_TOTAL=$((PARTS_TOTAL + 1)); fi
fi

# A part is over: the board shows «k/N parts» (its file is HERDR_PROJECTS_PROGRESS; the words are docs/check-progress.md).
# Several parts end at once, so the count is taken under a lock and the file is replaced in one go.
done_part() {
  [ -n "${HERDR_PROJECTS_PROGRESS:-}" ] || return 0
  (
    flock 8 || exit 0
    printf '%s\n' "$1" >>"$OUT/done"
    n=$(wc -l <"$OUT/done" | tr -d ' ')
    if printf '%s/%s parts\n' "$n" "$PARTS_TOTAL" >"$HERDR_PROJECTS_PROGRESS.new"; then
      mv "$HERDR_PROJECTS_PROGRESS.new" "$HERDR_PROJECTS_PROGRESS" || true
    fi
  ) 8>"$OUT/.lock"
}
# From the first moment the board knows how many parts this run has.
if [ -n "${HERDR_PROJECTS_PROGRESS:-}" ]; then
  if printf '0/%s parts\n' "$PARTS_TOTAL" >"$HERDR_PROJECTS_PROGRESS.new"; then
    mv "$HERDR_PROJECTS_PROGRESS.new" "$HERDR_PROJECTS_PROGRESS" || true
  fi
fi

# The Galaxy: sections split in four, each with a fresh board (a section that needs another's state is a bug).
# One screen (SECTIONS) is a single part: one fixture, one server, one browser.
n=0
if [ -n "${SECTIONS:-}" ]; then
  start_server "sections"
  job_slot
  ( set +e
    FIXTURE="$D" HERDR_PLANNER_TARGET_DIR="$TARGET" ONLY="$SECTIONS" limited node scripts/ui-check.mjs "http://127.0.0.1:$P" ${SHOTS:+"$SHOTS"} >"$OUT/sections.txt" 2>&1
    rc=$?; done_part sections; job_release; exit "$rc"
  ) &
  JOBS="$JOBS $!:sections"
else
  for only in "boot,live,navigator,buttons,autopilot,manual,stack,queue,search" \
              "chat,todo,hints" \
              "lag,universes,framing,remote,scale,runners,menu card" \
              "station,strays,crew,ship,theme,simple,planet states,narrow,top cards" \
              "focus,agent screen,requests,links,archive,load,top bar,limits,memory,layout narrow" \
              "layout 1280" "layout 1920,layout 1600" "layout 2560" "layout 1366,layout big"; do
    n=$((n + 1))
    want "ui$n" || continue
    start_server "ui$n"
    job_slot
    ( set +e
      t0=$(date +%s)
      FIXTURE="$D" HERDR_PLANNER_TARGET_DIR="$TARGET" ONLY="$only" limited node scripts/ui-check.mjs "http://127.0.0.1:$P" ${SHOTS:+"$SHOTS"} >"$OUT/ui$n.txt" 2>&1
      rc=$?; echo "(ui$n took $(($(date +%s) - t0)) s)" >>"$OUT/ui$n.txt"; done_part "ui$n"; job_release; exit "$rc"
    ) &
    JOBS="$JOBS $!:ui$n"
  done
fi

# A big world: 50 universes / 500 planets must stay orderly and cheap (the scale fixture).
if [ -z "${SECTIONS:-}" ] && want big; then
  start_server big --scale 50x500
  job_slot
  ( set +e
    FIXTURE="$D" limited node scripts/perf-check.mjs "http://127.0.0.1:$P" ${SHOTS:+"$SHOTS"} >"$OUT/big.txt" 2>&1
    rc=$?; done_part big; job_release; exit "$rc"
  ) &
  JOBS="$JOBS $!:big"
fi

# A server with a password: the login, the cookie and the limit; the ship's computer behind it.
if [ -z "${SECTIONS:-}" ] && want auth; then
  PASSWORD=secret start_server auth
  AUTH_D="$D"; AUTH_P="$P"
  job_slot
  (
    set +e
    # before auth-check's limit test: a wrong old password is refused in the server's words
    SHIP_LOGIN="me:secret" ONLY="ship login" FIXTURE="$AUTH_D" limited node scripts/ui-check.mjs "http://127.0.0.1:$AUTH_P" ${SHOTS:+"$SHOTS"} &&
      FIXTURE="$AUTH_D" AUTH_BASE="http://127.0.0.1:$AUTH_P" limited node scripts/auth-check.mjs
    rc=$?; done_part auth; job_release; exit "$rc"
  ) >"$OUT/auth.txt" 2>&1 &
  JOBS="$JOBS $!:auth"
fi

FAILED=
for j in $JOBS; do
  pid="${j%%:*}"; name="${j#*:}"
  if wait "$pid"; then :; else FAILED="$FAILED $name"; fi
  echo "══ $name"
  cat "$OUT/$name.txt"
done
if [ -n "$FAILED" ]; then echo "run-checks: failed:$FAILED"; exit 1; fi
# A section that failed once and passed on its second go (ui-check.mjs says so with a RETRIED line): the run is green, and
# the summary says plainly that it was not clean, so a flaky check is seen and fixed instead of being forgotten.
RETRIED="$(grep -h '^RETRIED' "$OUT"/*.txt 2>/dev/null || true)"
if [ -n "$RETRIED" ]; then
  echo "run-checks: ok, but with a second go for some page checks (flaky, to be fixed):"
  echo "$RETRIED"
  [ -z "${GITHUB_ACTIONS:-}" ] || printf '%s\n' "$RETRIED" | while IFS= read -r l; do echo "::warning title=Flaky page check::$l"; done
  exit 0
fi
echo "run-checks: ok"
