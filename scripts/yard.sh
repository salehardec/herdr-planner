#!/bin/sh
# Builds and checks on the shipyard (the machine with Docker, see docs and the planner's «Фокус · верфь»): one container
# per task, the working copy copied in by rsync through `docker exec`, the same scripts as here, the same exit code.
#
#   scripts/yard.sh up                        the image, the cache volume and the task's container
#   scripts/yard.sh sync                      up + the working copy into the container
#   scripts/yard.sh run <команда…>            up + sync + the command in /work/src (under the container's lock)
#   scripts/yard.sh build [ключи…]            cargo build
#   scripts/yard.sh test [ключи…]             cargo test
#   scripts/yard.sh web [--test|--check]      scripts/web-build.sh
#   scripts/yard.sh section <имена>           one fixture, one server, one browser: only these sections of ui-check
#                                             («runners todo»), screenshots in $TMPDIR/yard/out
#   scripts/yard.sh pages [части…]            parts of the page checks (ui2 big auth …; default: all), screenshots too
#   scripts/yard.sh check                     the whole scripts/run-checks.sh (≈ 6–7 min): what the board runs
#   scripts/yard.sh quick                     web tests and types, cargo test and build, the installer and the window,
#                                             without the page (≈ 1–1,5 min): what the board runs again on later turns
#   scripts/yard.sh fetch                     /work/out (screenshots, the parts' text) → $TMPDIR/yard/out
#   scripts/yard.sh down                      remove the task's container (the cache volume stays)
#   scripts/yard.sh status                    the container, the lock, the cache, the image
#   scripts/yard.sh prune [-y]                old images of this yard; ask before anything else
#
# Where the yard is: $DOCKER_HOST (the agents' rule exports it), else the default runner of
# ~/.herdr-projects/settings/runners.json, as ssh://<target>. YARD=local runs the same work on this machine, without a
# container (a way out for the human; an agent whose yard does not answer stops instead, see the board's rule).
#
# The task's tag: $HERDR_PROJECTS_TAG, else the name of $TMPDIR (…/PLN-49), else $HERDR_PROJECTS_ID with the repository's
# key from ~/.herdr-projects/settings/config.json, else dev-<user>. The container is yard-<tag>, labelled herdr.task=<tag>;
# the cache volume is hp-yard-cache with herdr.shared=herdr-planner, so the planner shows it apart from other people's
# containers and never offers it as forgotten.
set -eu
cd "$(dirname "$0")/.."
SELF=scripts/yard.sh
D=${YARD_DOCKER:-docker}
DFILE=scripts/yard/Dockerfile
IMAGE_NAME=herdr-planner-yard
VOLUME=${YARD_CACHE_VOLUME:-hp-yard-cache}
CPUS=${YARD_CPUS:-8}
MEM=${YARD_MEM:-12g}
SLOTS=${YARD_SLOTS:-2}
CACHE_MAX_GB=${YARD_CACHE_MAX_GB:-15}
LOG_DIR=${HERDR_YARD_LOG_DIR:-${TMPDIR:-/tmp}/yard}
PG=
# A progress poller left behind by an interrupted run would keep asking the container every two seconds.
trap '[ -z "$PG" ] || kill "$PG" 2>/dev/null || true' EXIT

usage() {
  sed -n '2,29p' "$SELF" | sed 's/^# \{0,1\}//'
}

say() { printf 'yard: %s\n' "$*"; }
die() { code=$1; shift; printf 'yard: %s\n' "$*" >&2; exit "$code"; }

# The same words the agents' rule uses when the yard does not answer.
down_yard() {
  printf 'yard: верфь %s не отвечает (DOCKER_HOST=%s).\n' "$YARD_SHOW" "${DOCKER_HOST:-не задан}" >&2
  printf 'yard: проверьте вход: ssh -o BatchMode=yes %s true\n' "$YARD_SSH" >&2
  printf 'yard: локально проверки не запускаются: дождитесь верфи (YARD=local — только для человека).\n' >&2
  exit 3
}

valid_tag() {
  printf '%s' "$1" | grep -Eq '^[A-Z][A-Z0-9]{1,4}-[0-9]+$'
}

task_tag() {
  if [ -n "${HERDR_PROJECTS_TAG:-}" ] && valid_tag "$HERDR_PROJECTS_TAG"; then
    printf '%s' "$HERDR_PROJECTS_TAG"
    return
  fi
  base=${TMPDIR:-}
  base=${base%/}
  name=${base##*/}
  if valid_tag "$name"; then
    printf '%s' "$name"
    return
  fi
  if [ -n "${HERDR_PROJECTS_ID:-}" ]; then
    repo=${HERDR_PROJECTS_ID%%/*}
    n=${HERDR_PROJECTS_ID##*/}
    key=
    cfg=$HOME/.herdr-projects/settings/config.json
    if [ -n "$repo" ] && [ -r "$cfg" ] && command -v jq >/dev/null 2>&1; then
      key=$(jq -r --arg r "$repo" '.repos[$r].repo.key // empty' "$cfg" 2>/dev/null || true)
    fi
    if [ -n "$key" ] && valid_tag "$key-$n"; then
      printf '%s-%s' "$key" "$n"
      return
    fi
  fi
  printf 'dev-%s' "${USER:-$(id -un)}"
}

sha12() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -c1-12
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -c1-12
  else
    die 2 "yard: нужен sha256sum (или shasum): по хешу Dockerfile выбирается образ"
  fi
}

# The yard: $DOCKER_HOST, else the default of the planner's own list of runners.
resolve_yard() {
  if [ -z "${DOCKER_HOST:-}" ]; then
    f=$HOME/.herdr-projects/settings/runners.json
    target=
    if [ -r "$f" ] && command -v jq >/dev/null 2>&1; then
      id=$(jq -r '.default // empty' "$f" 2>/dev/null || true)
      [ -z "$id" ] || target=$(jq -r --arg id "$id" '.runners[] | select(.id == $id) | .target' "$f" 2>/dev/null | head -n 1)
      [ -n "$target" ] || target=$(jq -r '.runners[0].target // empty' "$f" 2>/dev/null || true)
    fi
    if [ -z "$target" ]; then
      die 3 "yard: верфь не задана: нет DOCKER_HOST и нет верфи по умолчанию в ~/.herdr-projects/settings/runners.json"
    fi
    case "$target" in
      *://*) DOCKER_HOST=$target ;;
      *) DOCKER_HOST="ssh://$target" ;;
    esac
    export DOCKER_HOST
  fi
  YARD_SHOW=${DOCKER_HOST#ssh://}
  YARD_SSH=${YARD_SHOW%%/*}
}

ensure_yard() {
  if ! $D version --format '{{.Server.Version}}' >/dev/null 2>&1; then
    down_yard
  fi
}

# ───────── the container ─────────

image_tag() { printf '%s:%s' "$IMAGE_NAME" "$(sha12 "$DFILE")"; }

ensure_image() {
  IMAGE=$(image_tag)
  if $D image inspect "$IMAGE" >/dev/null 2>&1; then
    return
  fi
  say "собираю образ $IMAGE (первый раз на верфи — около двух минут)"
  $D build --label herdr.shared=herdr-planner -t "$IMAGE" - <"$DFILE" >/dev/null ||
    die 1 "yard: образ $IMAGE не собрался (см. вывод docker build выше)"
}

ensure_volume() {
  $D volume inspect "$VOLUME" >/dev/null 2>&1 && return
  $D volume create --label herdr.shared=herdr-planner "$VOLUME" >/dev/null
}

container_up() {
  ensure_image
  ensure_volume
  want=$($D image inspect -f '{{.Id}}' "$IMAGE")
  if $D container inspect "$NAME" >/dev/null 2>&1; then
    have=$($D inspect -f '{{.Image}}' "$NAME")
    if [ "$have" != "$want" ]; then
      # A new Dockerfile is a new image: the task's container is made again (its sources come back by sync).
      say "образ верфи новый: контейнер $NAME пересоздаётся"
      $D rm -f "$NAME" >/dev/null
    fi
  fi
  if ! $D container inspect "$NAME" >/dev/null 2>&1; then
    $D run -d --name "$NAME" --label "herdr.task=$TAG" --init --cpus "$CPUS" --memory "$MEM" --pids-limit 4096 \
      --security-opt no-new-privileges -v "$VOLUME:/cache" \
      -e CARGO_TARGET_DIR=/work/target -e HERDR_PLANNER_TARGET_DIR=/work/target \
      -e CARGO_HOME=/cache/cargo -e npm_config_cache=/cache/npm -e XDG_CACHE_HOME=/work/cache \
      -e RUN_CHECKS_JOBS=$((CPUS / 2)) -e CHROMIUM=/opt/chrome/chrome \
      "$IMAGE" sleep infinity >/dev/null ||
      die 1 "yard: контейнер $NAME не запустился"
  else
    running=$($D inspect -f '{{.State.Running}}' "$NAME")
    [ "$running" = "true" ] || $D start "$NAME" >/dev/null
  fi
}

sync_tree() {
  command -v rsync >/dev/null 2>&1 || die 2 "yard: нужен rsync на этой машине (исходники едут через docker exec)"
  # -c checks contents, never times: a file that changed gets the copy's time, always newer than the last build. The
  # .gitignore filter keeps target/ and node_modules/ out; .git is excluded by hand (the filter alone does not take it).
  rsync -rlc --delete --exclude .git --filter=':- .gitignore' -e "$D exec -i" ./ "$NAME:/work/in/" ||
    die 1 "yard: не удалось скопировать рабочую копию в $NAME"
}

# ───────── running ─────────

# The container side, run under flock /work/.lock: the copy /work/in → /work/src and the command. A full run (pages,
# check) also takes one of the shared slots of the yard, so three tasks do not fight over sixteen cores; while it waits,
# the board's progress file says so.
INNER=$(cat <<'INNER_EOF'
set -eu
if [ "${YARD_SLOT_USE:-0}" = 1 ]; then
  slots=${YARD_SLOTS:-2}
  locked=
  while [ -z "$locked" ]; do
    i=1
    busy=0
    while [ "$i" -le "$slots" ]; do
      fd=$((2 + i))
      eval "exec $fd>\"/cache/slots/$i\""
      if flock -n "$fd"; then
        locked=$i
        break
      fi
      eval "exec $fd>&-"
      busy=$((busy + 1))
      i=$((i + 1))
    done
    if [ -z "$locked" ]; then
      if [ -n "${HERDR_PROJECTS_PROGRESS:-}" ]; then
        printf '{"state":"waiting","ahead":%s}\n' "$busy" > "$HERDR_PROJECTS_PROGRESS.new"
        mv "$HERDR_PROJECTS_PROGRESS.new" "$HERDR_PROJECTS_PROGRESS"
      fi
      sleep 3
    fi
  done
fi
rsync -a --delete /work/in/ /work/src/
cd /work/src
# The debug binary finds the page when build.rs runs: a fresh container has no page yet (the sources never carry it), so
# the first command in it builds the page. Later commands see it and pay nothing.
if [ ! -f "${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/web/dist/index.html" ] && [ ! -f web/dist/index.html ]; then
  sh scripts/web-build.sh
fi
exec "$@"
INNER_EOF
)

mirror_progress() {
  [ -n "${HERDR_PROJECTS_PROGRESS:-}" ] || return 0
  tmp="$HERDR_PROJECTS_PROGRESS.new"
  if $D exec "$NAME" cat /work/out/progress 2>/dev/null >"$tmp"; then
    if [ -s "$tmp" ]; then mv "$tmp" "$HERDR_PROJECTS_PROGRESS"; else rm -f "$tmp"; fi
  else
    rm -f "$tmp"
  fi
}

progress_loop() {
  while :; do
    mirror_progress || true
    sleep 2
  done
}

# The command's output goes to the terminal and to a log (the board keeps only the tail; the agent has the whole file).
run_logged() {
  log=$1
  shift
  rcfile=$log.rc
  rm -f "$rcfile"
  set +e
  { "$@"; printf '%s' "$?" >"$rcfile"; } 2>&1 | tee "$log"
  rc=$(cat "$rcfile" 2>/dev/null || true)
  rm -f "$rcfile"
  set -e
  return "${rc:-1}"
}

# slot: 1 for a full page run (pages, check), 0 for everything else.
run_here() {
  slot=$1
  logname=$2
  shift 2
  log="$LOG_DIR/$(date +%Y%m%d-%H%M%S)-$logname.log"
  if [ -n "${HERDR_PROJECTS_PROGRESS:-}" ]; then
    $D exec "$NAME" sh -c 'rm -f /work/out/progress' >/dev/null 2>&1 || true
    progress_loop &
    PG=$!
  fi
  set +e
  if [ "$slot" = 1 ]; then
    run_logged "$log" "$D" exec -i -w /work/src -e "YARD_SLOTS=$SLOTS" -e YARD_SLOT_USE=1 "$NAME" \
      flock /work/.lock sh -c "$INNER" yard "$@"
  else
    run_logged "$log" "$D" exec -i -w /work/src -e YARD_SLOT_USE=0 "$NAME" \
      flock /work/.lock sh -c "$INNER" yard "$@"
  fi
  rc=$?
  set -e
  if [ -n "$PG" ]; then
    kill "$PG" 2>/dev/null || true
    wait "$PG" 2>/dev/null || true
    PG=
  fi
  mirror_progress || true
  return "$rc"
}

prepare_out() {
  $D exec "$NAME" sh -c 'rm -rf /work/out && mkdir -p /work/out' >/dev/null 2>&1 || true
}

fetch_out() {
  if $D exec "$NAME" sh -c 'test -d /work/out' >/dev/null 2>&1; then
    rm -rf "$LOG_DIR/out"
    $D cp "$NAME:/work/out" "$LOG_DIR/out" >/dev/null &&
      say "снимки и тексты частей: $LOG_DIR/out"
  fi
}

# ───────── lazy commands ─────────

up() {
  ensure_yard
  container_up
}

quick_script() {
  cat <<'QUICK_EOF'
set -e
sh scripts/web-build.sh --test
sh scripts/web-build.sh --check
cargo test --quiet
cargo build --quiet
sh scripts/check-install.sh "${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}/debug/herdr-planner"
sh scripts/check-open.sh
QUICK_EOF
}

page_command() { # slot, logname, then the argv
  prepare_out
  set +e
  run_here "$@"
  rc=$?
  set -e
  fetch_out || true
  return "$rc"
}

# ───────── local mode ─────────

local_work() {
  target="${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}"
  mkdir -p "$LOG_DIR/out/shots"
  case "$1" in
    build) shift; exec cargo build "$@" ;;
    test) shift; exec cargo test "$@" ;;
    web) shift; exec sh scripts/web-build.sh "$@" ;;
    run) shift; exec "$@" ;;
    section)
      names=${2:-}
      [ -n "$names" ] || die 2 "yard: section: назовите секции, например «runners todo»"
      SECTIONS=$names exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    pages)
      parts=${2:-}
      PAGE_ONLY=1 SHARDS=$parts exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    check) exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    quick) HERDR_PLANNER_TARGET_DIR=$target CARGO_TARGET_DIR=$target exec sh -c "$(quick_script)" ;;
    fetch) say "YARD=local: снимки и тексты — $LOG_DIR/out" ;;
    down|up|sync|status|prune) say "YARD=local: контейнера нет, всё на этой машине" ;;
    *) usage; exit 2 ;;
  esac
}

# ───────── the commands ─────────

cmd=${1:-help}
if [ $# -gt 0 ]; then shift; fi

case "$cmd" in
  help|-h|--help) usage; exit 0 ;;
  build|test|web|run|section|pages|check|quick|fetch|down|up|sync|status|prune) ;;
  *) usage; exit 2 ;;
esac

mkdir -p "$LOG_DIR"

if [ "${YARD:-}" = local ]; then
  local_work "$cmd" "$@"
fi

TAG=$(task_tag)
NAME="yard-$TAG"
IMAGE=$(image_tag)
resolve_yard

case "$cmd" in
  up) up ;;
  sync) up; sync_tree; say "рабочая копия в $NAME:/work/in" ;;
  run)
    [ $# -gt 0 ] || die 2 "yard: run: нужна команда, например «sh scripts/run-checks.sh»"
    up; sync_tree; run_here 0 run "$@" ;;
  build) up; sync_tree; run_here 0 build cargo build "$@" ;;
  test) up; sync_tree; run_here 0 test cargo test "$@" ;;
  web) up; sync_tree; run_here 0 web sh scripts/web-build.sh "$@" ;;
  section)
    names=${1:-}
    [ -n "$names" ] || die 2 "yard: section: назовите секции, например «runners todo»"
    up; sync_tree
    page_command 0 section env SECTIONS="$names" HERDR_PROJECTS_PROGRESS=/work/out/progress \
      sh scripts/run-checks.sh /work/out/shots ;;
  pages)
    parts=${1:-}
    up; sync_tree
    page_command 1 pages env PAGE_ONLY=1 SHARDS="$parts" HERDR_PROJECTS_PROGRESS=/work/out/progress \
      sh scripts/run-checks.sh /work/out/shots ;;
  check)
    up; sync_tree
    page_command 1 check env HERDR_PROJECTS_PROGRESS=/work/out/progress \
      sh scripts/run-checks.sh /work/out/shots ;;
  quick)
    up; sync_tree
    run_here 0 quick env HERDR_PLANNER_TARGET_DIR=/work/target CARGO_TARGET_DIR=/work/target \
      sh -c "$(quick_script)" ;;
  fetch) ensure_yard; fetch_out ;;
  down)
    ensure_yard
    if $D container inspect "$NAME" >/dev/null 2>&1; then
      $D rm -f -v "$NAME" >/dev/null
      say "контейнер $NAME убран; кэш $VOLUME остался"
    else
      say "контейнера $NAME нет"
    fi ;;
  status)
    ensure_yard
    say "верфь $YARD_SHOW, задача $TAG, образ $IMAGE"
    if $D container inspect "$NAME" >/dev/null 2>&1; then
      $D ps --filter "name=^${NAME}$" --format 'контейнер {{.Names}}: {{.Status}}'
      # shellcheck disable=SC2016  # $(du) is for the container's shell, not for this one
      $D exec "$NAME" sh -c 'if flock -n /work/.lock -c true; then echo "замок: свободен"; else echo "замок: занят"; fi; echo "кэш: $(du -sh /cache 2>/dev/null | cut -f1)"' 2>/dev/null || true
    else
      say "контейнера $NAME нет (up создаст)"
    fi ;;
  prune)
    ensure_yard
    yes=
    [ "${1:-}" = "-y" ] && yes=1
    current=${IMAGE#*:}
    stale=$($D image ls --filter label=herdr.shared=herdr-planner --format '{{.Repository}}:{{.Tag}}' |
      grep "^$IMAGE_NAME:" | grep -v ":$current$" || true)
    if [ -n "$stale" ]; then
      say "старые образы верфи:"
      printf '%s\n' "$stale" | sed 's/^/  /'
      if [ -n "$yes" ]; then
        printf '%s\n' "$stale" | while IFS= read -r img; do $D image rm "$img" >/dev/null 2>&1 || true; done
        say "убраны"
      else
        say "убрать их: sh $SELF prune -y"
      fi
    else
      say "старых образов верфи нет"
    fi
    if $D container inspect "$NAME" >/dev/null 2>&1; then
      size_kb=$($D exec "$NAME" du -sk /cache 2>/dev/null | cut -f1 || echo 0)
      if [ "${size_kb:-0}" -gt $((CACHE_MAX_GB * 1024 * 1024)) ] 2>/dev/null; then
        say "кэш $VOLUME больше $CACHE_MAX_GB ГБ ($((size_kb / 1024 / 1024)) ГБ): следующая холодная сборка — около 40 с"
        if [ -n "$yes" ]; then
          $D rm -f -v "$NAME" >/dev/null
          $D volume rm "$VOLUME" >/dev/null && say "том $VOLUME убран (контейнер тоже)"
        else
          say "убрать контейнер и том: sh $SELF prune -y (или «Очистить кэш сборок» в планировщике)"
        fi
      fi
    fi ;;
esac
