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
#                                             («runners todo»); the page and the binary are built first, screenshots in
#                                             $TMPDIR/yard/out
#   scripts/yard.sh pages [части…]            parts of the page checks (ui2 big auth …; default: all), built first too
#   scripts/yard.sh check                     the whole scripts/run-checks.sh (≈ 13 мин): what the board runs
#   scripts/yard.sh quick                     web tests and types, cargo test and build, the installer and the window,
#                                             without the page (≈ 1–1,5 min): what the board runs again on later turns
#   scripts/yard.sh fetch                     the run's folder (screenshots, the parts' text) → $TMPDIR/yard/out
#   scripts/yard.sh down                      remove the task's container (the cache volume stays)
#   scripts/yard.sh status                    the container, the lock, the cache, the image
#   scripts/yard.sh prune [-y]                old images of this yard; ask before anything else
#   scripts/yard.sh where                     which yard this task would use, and nothing else
#
# Where the yard is: $DOCKER_HOST (the agents' rule exports it), else the rule of the task or its repository in
# ~/.herdr-projects/settings/runners.json (a named yard, or tags like gpu/x86_64), else that list's default. A yard that
# does not answer, or whose load1 is at its busy threshold, gives way to the next one the rule allows; when all are busy,
# the least loaded is taken. YARD_RUNNER names one yard outright. YARD=local runs the same work on this machine, without
# a container (a way out for the human; an agent whose yard does not answer stops instead, see the board's rule).
#
# The task's container: `--cpus 6 --memory 12g` and RUN_CHECKS_JOBS=3 by default (PLN-58: two full checks on sixteen cores
# take twelve, the machine keeps four). A yard labelled «gpu» whose `nvidia-smi` answers gets its card into the container
# (`--gpus all`, the graphics capabilities, /dev/dri when it is there); the page checks then draw with the GPU instead of
# SwiftShader. A full run (pages, check) takes one of the yard's shared slots — two by default, the rest wait and say how
# many runs are ahead. The yard's own check_cpus/check_slots in runners.json (or YARD_CPUS/YARD_SLOTS/YARD_SLOT_USE here)
# say otherwise; see «Фокус · верфь» in the planner.
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
# The check container's CPU limit and the full runs allowed at once: the yard's own from the planner's list (PLN-58)
# once it is known, else 6 and 2; the variables here win as they always did.
CPUS=${YARD_CPUS:-}
MEM=${YARD_MEM:-12g}
SLOTS=${YARD_SLOTS:-}
SLOT_USE=${YARD_SLOT_USE:-1}
GPU_FLAGS=
CACHE_MAX_GB=${YARD_CACHE_MAX_GB:-15}
# The container's log is the journal of its commands (PLN-56) and is bounded: three files of eight megabytes at most,
# the oldest dropped by docker itself, so a container that lives for weeks cannot fill the yard's disk.
LOG_MAX=${YARD_LOG_MAX:-8m}
LOG_FILES=${YARD_LOG_FILES:-3}
LOG_DIR=${HERDR_YARD_LOG_DIR:-${TMPDIR:-/tmp}/yard}
PG=
# One run of the wrapper, one name inside the container: its progress file and its result folder are its own, so a second
# run of the same task cannot overwrite them.
RUN_ID=${YARD_RUN_ID:-$(date +%Y%m%d-%H%M%S)-$$}
# A progress poller left behind by an interrupted run would keep asking the container every two seconds.
trap '[ -z "$PG" ] || kill "$PG" 2>/dev/null || true' EXIT

usage() {
  sed -n '2,35p' "$SELF" | sed 's/^# \{0,1\}//'
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

# The repository of the task's tag, when the board's settings know its key (`config.json` first, then `repo-keys.json`).
repo_of_tag() {
  key=$1
  [ -n "$key" ] || return 0
  cfg=$HOME/.herdr-projects/settings/config.json
  if [ -r "$cfg" ]; then
    jq -r --arg k "$key" '.repos | to_entries[]? | select((.value.repo.key? // "") == $k) | .key' "$cfg" 2>/dev/null | head -n 1
    return 0
  fi
  keys=$HOME/.herdr-projects/data/repo-keys.json
  if [ -r "$keys" ]; then
    jq -r --arg k "$key" '(.had // .) | to_entries[]? | select(.value == $k) | .key' "$keys" 2>/dev/null | head -n 1
  fi
}

# The yards the rule allows, in the order they are tried: `id, target, busy %, tags`. The task's own rule replaces the
# repository's; a named yard comes first, then the default, then the list.
yard_candidates() { # file, task id, repo, wanted yard (empty — the rule)
  jq -r --arg task "$2" --arg repo "$3" --arg want "$4" '
    . as $f
    | ($f.tasks[$task] // null) as $t
    | (($t != null) and ((($t.runner // "") != "") or ((($t.tags // []) | length) > 0))) as $tany
    | (if $tany then ($t.tags // []) else ($f.need[$repo] // []) end) as $need
    | (if $tany then ($t.runner // "") else ($f.assign[$repo] // "") end) as $pref
    | reduce ([
        ($f.runners[] | select(.id == $pref)),
        ($f.runners[] | select(.id == ($f.default // ""))),
        $f.runners[]
      ][]) as $r ([]; if any(.id == $r.id) then . else . + [$r] end)
    | .[] as $r
    | select($want == "" or $r.id == $want)
    | select(($need | length) == 0 or ($need | all(. as $n | (($r.tags // []) | map(ascii_downcase) | index($n | ascii_downcase)) != null)))
    | [$r.id, $r.target, ($r.busy_pct // $f.busy_pct // 85), (($r.tags // []) | join(","))] | @tsv
  ' "$1" 2>/dev/null
}

# `load1 cpus` of a yard over ssh; nothing for a yard that does not answer.
ssh_load() {
  t=$1
  case "$t" in *://*) t=${t#*://} ;; esac
  port=
  case "$t" in
    *:* ) port=${t##*:}; t=${t%:*} ;;
  esac
  if [ -n "$port" ]; then
    ssh -o BatchMode=yes -o ConnectTimeout=5 -p "$port" "$t" 'echo "$(cut -d" " -f1 /proc/loadavg) $(nproc)"' 2>/dev/null
  else
    ssh -o BatchMode=yes -o ConnectTimeout=5 "$t" 'echo "$(cut -d" " -f1 /proc/loadavg) $(nproc)"' 2>/dev/null
  fi
}

# The yard: $DOCKER_HOST; else the task's (or its repository's) rule from the planner's list of runners; else the default
# of that list. A yard that does not answer, or is loaded above its threshold, gives way to the next one the rule allows;
# when every allowed yard is busy, the least loaded is taken rather than a yard the rule does not allow.
resolve_yard() {
  if [ -n "${YARD_RUNNER:-}" ] && [ -z "${DOCKER_HOST:-}" ]; then
    f=$HOME/.herdr-projects/settings/runners.json
    [ -r "$f" ] || die 3 "yard: YARD_RUNNER=$YARD_RUNNER, а списка верфей нет: $f"
    command -v jq >/dev/null 2>&1 || die 3 "yard: для выбора верфи нужен jq"
    target=$(jq -r --arg id "$YARD_RUNNER" '.runners[] | select(.id == $id) | .target' "$f" 2>/dev/null | head -n 1)
    [ -n "$target" ] || die 3 "yard: верфи $YARD_RUNNER нет в $f"
    case "$target" in *://*) DOCKER_HOST=$target ;; *) DOCKER_HOST="ssh://$target" ;; esac
    export DOCKER_HOST
  elif [ -z "${DOCKER_HOST:-}" ]; then
    f=$HOME/.herdr-projects/settings/runners.json
    target=
    if [ -r "$f" ] && command -v jq >/dev/null 2>&1; then
      key=${TAG%%-*}
      n=${TAG##*-}
      repo=
      [ "$n" != "$TAG" ] && repo=$(repo_of_tag "$key")
      task=
      [ -n "$repo" ] && task="$repo/$n"
      cands=$(yard_candidates "$f" "$task" "$repo" "${YARD_RUNNER:-}")
      if [ -z "$cands" ]; then
        need=$(jq -r --arg task "$task" --arg repo "$repo" '
          . as $f
          | ($f.tasks[$task] // null) as $t
          | (($t != null) and ((($t.runner // "") != "") or ((($t.tags // []) | length) > 0))) as $any
          | (if $any then ($t.tags // []) else ($f.need[$repo] // []) end) | join(", ")
        ' "$f" 2>/dev/null || true)
        die 3 "yard: в $f нет верфи под правило задачи ${task:-$TAG} (нужные метки: ${need:-нет})"
      fi
      best=
      low=
      low_load=
      seen=
      while IFS="$(printf '\t')" read -r id addr pct tags; do
        [ -n "$id" ] || continue
        seen="$seen $id${tags:+[$tags]}"
        probe=$(ssh_load "$addr" || true)
        [ -n "$probe" ] || continue
        load1=${probe% *}
        cpus=${probe##* }
        if [ -n "$pct" ] && [ "$pct" != 0 ] && [ -n "$cpus" ] && [ "$cpus" != 0 ] &&
          awk -v l="$load1" -v c="$cpus" -v p="$pct" 'BEGIN { exit !(l * 100 / c >= p) }'; then
          # Busy: remember the least loaded of the busy ones as the last way out.
          if [ -z "$low" ] || awk -v a="$load1" -v b="$low_load" 'BEGIN { exit !(a < b) }'; then
            low=$addr
            low_load=$load1
          fi
          continue
        fi
        best=$addr
        break
      done <<YARD_LIST
$cands
YARD_LIST
      [ -n "$best" ] || best=$low
      if [ -z "$best" ]; then
        die 3 "yard: ни одна подходящая верфь не отвечает (проверены:$seen)"
      fi
      target=$best
      if [ -n "$low" ] && [ "$best" = "$low" ]; then
        printf 'yard: все подходящие верфи заняты — беру наименее загруженную %s\n' "$best" >&2
      fi
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

# ───────── the yard's own settings (PLN-58) ─────────

# One command on the yard itself over ssh (the machine behind $DOCKER_HOST=ssh://…); nothing when it cannot be asked.
yard_ssh() {
  t=$YARD_SSH
  port=
  case "$t" in
    *:* ) port=${t##*:}; t=${t%:*} ;;
  esac
  if [ -n "$port" ]; then
    ssh -o BatchMode=yes -o ConnectTimeout=5 -p "$port" "$t" "$@" 2>/dev/null
  else
    ssh -o BatchMode=yes -o ConnectTimeout=5 "$t" "$@" 2>/dev/null
  fi
}

# The resolved yard's entry in the planner's list, as `tags, check_cpus, check_slots`; nothing when the list is missing,
# the yard is not in it, or there is no jq. The target is matched without its scheme, its user and its port.
yard_rule() {
  f=$HOME/.herdr-projects/settings/runners.json
  [ -r "$f" ] && command -v jq >/dev/null 2>&1 || return 0
  jq -r --arg host "$YARD_SSH" '
    def h: sub("^[A-Za-z][A-Za-z0-9+.-]*://"; "") | sub("^[^@]*@"; "") | sub(":[0-9]+$"; "");
    (.runners // [])[] | select(((.target // "") | h) == ($host | h))
    | [((.tags // []) | join(",")), (.check_cpus // ""), (.check_slots // "")] | @tsv
  ' "$f" 2>/dev/null | head -n 1
}

# A yard gives its card to the container when the list labels it «gpu» and the driver on the yard answers: the label is a
# promise, `nvidia-smi` is the proof. Only an ssh yard can be asked, and only with ssh at hand.
yard_gpu() {
  printf '%s' "$YARD_TAGS" | tr '[:upper:]' '[:lower:]' | tr ',' '\n' | grep -qx gpu || return 1
  case "${DOCKER_HOST:-}" in ssh://*) ;; *) return 1 ;; esac
  command -v ssh >/dev/null 2>&1 || return 1
  yard_ssh 'nvidia-smi -L' >/dev/null 2>&1
}

# The CPU limit of the check container, the slots of full runs and the GPU flags: the list's word for this yard, else the
# defaults; YARD_CPUS, YARD_SLOTS and YARD_SLOT_USE here still win. A yard without a card runs as it always did.
yard_settings() {
  rule=$(yard_rule || true)
  YARD_TAGS=$(printf '%s' "$rule" | cut -f1)
  list_cpus=$(printf '%s' "$rule" | cut -f2)
  list_slots=$(printf '%s' "$rule" | cut -f3)
  CPUS=${YARD_CPUS:-${list_cpus:-6}}
  SLOTS=${YARD_SLOTS:-${list_slots:-2}}
  if [ "$SLOTS" = 0 ]; then SLOT_USE=0; fi
  if yard_gpu; then
    GPU_FLAGS='--gpus all -e NVIDIA_VISIBLE_DEVICES=all -e NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics'
    if yard_ssh 'test -e /dev/dri' >/dev/null 2>&1; then
      GPU_FLAGS="$GPU_FLAGS --device /dev/dri"
    fi
    say "верфь $YARD_SHOW: GPU в контейнере задачи (--gpus all), ЦП $CPUS, слотов $SLOTS"
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
      # A new Dockerfile is a new image: the task's container is made again (its sources and its build come back by
      # sync and the next build; nothing else is worth keeping alive across the image it was made from).
      say "образ верфи новый: контейнер $NAME пересоздаётся"
      $D rm -f "$NAME" >/dev/null
    fi
  fi
  if $D container inspect "$NAME" >/dev/null 2>&1; then
    # A container made before the journal has docker's default log: no bound. Its log cannot be bounded in place, so it
    # is made again when no run holds it (the sources and the build come back with the next command); a busy one waits.
    bounded=$($D inspect -f '{{.HostConfig.LogConfig.Type}} {{index .HostConfig.LogConfig.Config "max-size"}}' "$NAME" 2>/dev/null || true)
    case "$bounded" in
      json-file\ ?*) ;;
      *)
        if $D start "$NAME" >/dev/null 2>&1 && $D exec "$NAME" flock -n /work/.lock true >/dev/null 2>&1; then
          say "журнал контейнера $NAME без ограничения размера: пересоздаю с ним"
          $D rm -f "$NAME" >/dev/null
        else
          say "журнал контейнера $NAME без ограничения размера, но он занят: его пересоздаст следующий запуск"
        fi ;;
    esac
  fi
  if ! $D container inspect "$NAME" >/dev/null 2>&1; then
    gpu=0
    if [ -n "$GPU_FLAGS" ]; then gpu=1; fi
    cpus_flag=
    jobs_flag=
    if [ "$CPUS" != 0 ]; then
      cpus_flag="--cpus $CPUS"
      jobs_flag="-e RUN_CHECKS_JOBS=$((CPUS / 2))"
    fi
    # shellcheck disable=SC2086  # «--cpus 6» and «-e …» or nothing; the GPU flags are a few words of their own (PLN-58)
    $D run -d --name "$NAME" --label "herdr.task=$TAG" --label "herdr.yard.cpus=$CPUS" --label "herdr.yard.gpu=$gpu" \
      --init $cpus_flag --memory "$MEM" --pids-limit 4096 \
      --log-driver json-file --log-opt "max-size=$LOG_MAX" --log-opt "max-file=$LOG_FILES" \
      --security-opt no-new-privileges $GPU_FLAGS -v "$VOLUME:/cache" \
      -e CARGO_TARGET_DIR=/work/target -e HERDR_PLANNER_TARGET_DIR=/work/target \
      -e CARGO_HOME=/cache/cargo -e npm_config_cache=/cache/npm -e XDG_CACHE_HOME=/work/cache \
      $jobs_flag -e CHROMIUM=/opt/chrome/chrome \
      "$IMAGE" sleep infinity >/dev/null ||
      die 1 "yard: контейнер $NAME не запустился"
  else
    running=$($D inspect -f '{{.State.Running}}' "$NAME")
    [ "$running" = "true" ] || $D start "$NAME" >/dev/null
    # The CPU limit belongs to the container and docker changes it in place: a run of this task, /work/target and the
    # sources stay. The value here is this invocation's (the yard's check_cpus, or YARD_CPUS for one run). Docker cannot
    # lift a limit with `--cpus 0`, so a zero is read back and said as it is — the container is never thrown away for it.
    have_cpus=$($D inspect -f '{{.HostConfig.NanoCpus}}' "$NAME" 2>/dev/null || true)
    case "$have_cpus" in ''|'<no value>'|null) have_cpus=0 ;; esac
    want_cpus=$((CPUS * 1000000000))
    if [ "$have_cpus" != "$want_cpus" ]; then
      $D update --cpus "$CPUS" "$NAME" >/dev/null 2>&1 || true
      now_cpus=$($D inspect -f '{{.HostConfig.NanoCpus}}' "$NAME" 2>/dev/null || true)
      case "$now_cpus" in ''|'<no value>'|null) now_cpus=0 ;; esac
      if [ "$now_cpus" = "$want_cpus" ]; then
        say "контейнер $NAME: лимит ЦП $((have_cpus / 1000000000)) → $CPUS (на месте, контейнер не тронут)"
      else
        say "контейнер $NAME: лимит ЦП $((now_cpus / 1000000000)) остался (docker update не дал $CPUS); его сменит только пересоздание"
      fi
    fi
    # The card, once in the container, stays. A probe that fails once (a busy yard, a five-second timeout) is not a
    # reason to throw the container away — nor is a card that appeared later added to a live container: down and up do.
    have_gpu=$($D inspect -f '{{json .HostConfig.DeviceRequests}}' "$NAME" 2>/dev/null || true)
    case "$have_gpu" in ''|'<no value>'|null) have_gpu= ;; esac
    if [ -z "$GPU_FLAGS" ] && [ -n "$have_gpu" ]; then
      say "верфь сейчас не отвечает про карту: контейнер $NAME остаётся с ней, пока его не пересоздадут"
    elif [ -n "$GPU_FLAGS" ] && [ -z "$have_gpu" ]; then
      say "верфь даёт карту, а контейнер $NAME её не имеет: «sh $SELF down» и снова up добавят её"
    elif [ -n "$GPU_FLAGS" ]; then
      # A long-lived container that outlived a driver reload keeps stale device bindings: the yard's driver answers, the
      # container's card does not. The checks fall back to SwiftShader by themselves; the container is never thrown away
      # for it, and down/up are what bring the card back.
      if ! $D exec "$NAME" nvidia-smi -L >/dev/null 2>&1; then
        say "карта в контейнере $NAME не отвечает (nvidia-smi внутри): проверки пойдут на SwiftShader; «sh $SELF down» и снова up вернут карту"
      fi
    fi
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
# The task's journal is the container's own log (PLN-56): `docker logs`, so the planner's «Логи», shows what the commands
# printed. The main process is `sleep infinity`, a command arrives by `docker exec`; its output is copied to the main
# process's stdout (/proc/1/fd/1), after a header line and before a line with the exit code and the duration. The agent
# gets the output as it was: the journal is a copy, and the exit code is the command's own.
J=/proc/1/fd/1
[ -w "$J" ] || J=/dev/null
stamp() { date '+%Y-%m-%d %H:%M:%S'; }
span() {
  s=$1
  if [ "$s" -ge 3600 ]; then printf '%sh %sm %ss' $((s / 3600)) $((s % 3600 / 60)) $((s % 60))
  elif [ "$s" -ge 60 ]; then printf '%sm %ss' $((s / 60)) $((s % 60))
  else printf '%ss' "$s"; fi
}
body() {
  rsync -a --delete /work/in/ /work/src/
  cd /work/src
  # The debug binary finds the page when build.rs runs: a fresh container has no page yet (the sources never carry it),
  # so the first command in it builds the page. Later commands see it and pay nothing.
  if [ ! -f "${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/web/dist/index.html" ] && [ ! -f web/dist/index.html ]; then
    sh scripts/web-build.sh
  fi
  "$@"
}
label=$(printf '%.200s' "${YARD_LABEL:-$*}" | tr '\n\t' '  ')
begun=$(date +%s)
printf '[yard] ▶ %s · yard.sh %s · run %s\n' "$(stamp)" "$label" "${YARD_RUN:-—}" >>"$J"
rcf=/work/.rc.$$
rm -f "$rcf"
{ ( set -eu; body "$@" ); printf '%s' "$?" >"$rcf"; } 2>&1 | tee --output-error=warn-nopipe -a "$J" || true
rc=$(cat "$rcf" 2>/dev/null || true)
rm -f "$rcf"
rc=${rc:-1}
printf '[yard] ■ %s · yard.sh %s · exit %s · %s\n' "$(stamp)" "$label" "$rc" "$(span $(($(date +%s) - begun)))" >>"$J"
exit "$rc"
INNER_EOF
)

mirror_progress() {
  [ -n "${HERDR_PROJECTS_PROGRESS:-}" ] || return 0
  tmp="$HERDR_PROJECTS_PROGRESS.new"
  if $D exec "$NAME" cat "/work/progress.$RUN_ID" 2>/dev/null >"$tmp"; then
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

# The same in the background: the page run holds the lock until its folder is fetched, and the host must copy while it
# still holds it (set STREAM_PID; the exit code lands in <log>.rc, tee's own status says nothing).
start_logged() {
  log=$1
  shift
  rcfile="$log.rc"
  rm -f "$rcfile"
  { "$@"; printf '%s' "$?" >"$rcfile"; } 2>&1 | tee "$log" &
  STREAM_PID=$!
}

# Waits (one docker exec) for the page script's ready marker. The page script keeps the lock until this host leaves the
# fetched marker, so no run waiting behind it can sweep the folder before the copy. A free lock after the startup grace
# means the run ended without pages (a failure before them).
page_wait_fetch() { # run id
  run=$1
  dir="/work/out.$run"
  if ! $D exec "$NAME" sh -c "i=0; while :; do [ -e '$dir.ready' ] && exit 0; i=\$((i+1)); if [ \$i -gt 20 ] && flock -n /work/.lock -c true 2>/dev/null; then exit 1; fi; sleep 0.25; done"; then
    return 1
  fi
  rm -rf "$LOG_DIR/out"
  if $D cp "$NAME:$dir" "$LOG_DIR/out" >/dev/null; then
    say "снимки и тексты частей: $LOG_DIR/out"
  fi
  $D exec "$NAME" touch "$dir.fetched" >/dev/null 2>&1 || true
  return 0
}

# slot: 1 for a full page run (pages, check), 0 for everything else; progress: 1 when the command reports «k/N parts».
# The progress file lives in the container under the run's own name and is passed to the whole exec — the slot loop inside
# must see it too, or a run waiting for a slot says nothing.
run_here() {
  slot=$1
  logname=$2
  progress=$3
  shift 3
  log="$LOG_DIR/$(date +%Y%m%d-%H%M%S)-$logname.log"
  prog=
  if [ "$progress" = 1 ]; then prog="/work/progress.$RUN_ID"; fi
  if [ -n "$prog" ] && [ -n "${HERDR_PROJECTS_PROGRESS:-}" ]; then
    progress_loop &
    PG=$!
  fi
  set +e
  if [ "$progress" = 1 ]; then
    # The result folder is copied while the run still holds the lock; the run's own exit code comes from its .rc file.
    if [ "$slot" = 1 ]; then
      start_logged "$log" "$D" exec -i -w /work/src -e "HERDR_PROJECTS_PROGRESS=$prog" \
        -e "YARD_SLOTS=$SLOTS" -e YARD_SLOT_USE="$SLOT_USE" -e "UI_GPU=${UI_GPU:-}" -e "YARD_LABEL=$LABEL" -e "YARD_RUN=$RUN_ID" "$NAME" \
        flock /work/.lock sh -c "$INNER" yard "$@"
    else
      start_logged "$log" "$D" exec -i -w /work/src -e "HERDR_PROJECTS_PROGRESS=$prog" -e YARD_SLOT_USE=0 -e "UI_GPU=${UI_GPU:-}" -e "YARD_LABEL=$LABEL" -e "YARD_RUN=$RUN_ID" "$NAME" \
        flock /work/.lock sh -c "$INNER" yard "$@"
    fi
    execpid=$STREAM_PID
    page_wait_fetch "$RUN_ID"
    wait "$execpid"
    rc=$(cat "$log.rc" 2>/dev/null || true)
    rm -f "$log.rc"
  else
    if [ "$slot" = 1 ]; then
      run_logged "$log" "$D" exec -i -w /work/src -e "HERDR_PROJECTS_PROGRESS=$prog" \
        -e "YARD_SLOTS=$SLOTS" -e YARD_SLOT_USE="$SLOT_USE" -e "UI_GPU=${UI_GPU:-}" -e "YARD_LABEL=$LABEL" -e "YARD_RUN=$RUN_ID" "$NAME" \
        flock /work/.lock sh -c "$INNER" yard "$@"
    else
      run_logged "$log" "$D" exec -i -w /work/src -e "HERDR_PROJECTS_PROGRESS=$prog" -e YARD_SLOT_USE=0 -e "UI_GPU=${UI_GPU:-}" -e "YARD_LABEL=$LABEL" -e "YARD_RUN=$RUN_ID" "$NAME" \
        flock /work/.lock sh -c "$INNER" yard "$@"
    fi
    rc=$?
  fi
  set -e
  if [ -n "$PG" ]; then
    kill "$PG" 2>/dev/null || true
    wait "$PG" 2>/dev/null || true
    PG=
  fi
  mirror_progress || true
  if [ -n "$prog" ]; then $D exec "$NAME" rm -f "$prog" >/dev/null 2>&1 || true; fi
  return "$rc"
}

# The pages of a run: the host fetches the run's own folder (/work/out.<run>), which a later run of the same task cannot
# touch, and the container drops it afterwards.
fetch_out() {
  run=$1
  dir="/work/out.$run"
  if $D exec "$NAME" test -d "$dir" >/dev/null 2>&1; then
    rm -rf "$LOG_DIR/out"
    if $D cp "$NAME:$dir" "$LOG_DIR/out" >/dev/null; then
      say "снимки и тексты частей: $LOG_DIR/out"
      $D exec "$NAME" rm -rf "$dir" >/dev/null 2>&1 || true
    fi
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

# A page run under the lock: its own /work/out (a previous run's leftovers are dropped only by the run that holds the
# lock), the sources' page and binary built when asked, then run-checks writes its screenshots and part texts there.
page_script() { # build(0|1), run id
  if [ "$1" = 1 ]; then
    # shellcheck disable=SC2016  # $rc is for the container's shell, not for this one
    build='sh scripts/web-build.sh || rc=$?
if [ "$rc" -eq 0 ]; then cargo build --quiet || rc=$?; fi
'
  else
    build=''
  fi
  cat <<PAGE_END
rc=0
# Under the lock no other run of this task is going: its own /work/out and every older run's leftovers are safe to drop
# (a run killed before its pages were fetched leaves them; the next run sweeps them).
rm -rf /work/out /work/out.* /work/progress.*
mkdir -p /work/out
$build
if [ "\$rc" -eq 0 ]; then sh scripts/run-checks.sh /work/out/shots; rc=\$?; fi
mv /work/out "/work/out.$2" 2>/dev/null || true
# The host copies this folder while our lock is still held: it waits for .ready and leaves .fetched. A run that starts
# later waits for the lock, so it cannot sweep the folder from under the copy.
if [ -d "/work/out.$2" ]; then
  : > "/work/out.$2.ready"
  n=0
  while [ ! -e "/work/out.$2.fetched" ] && [ "\$n" -lt 300 ]; do sleep 0.2; n=\$((n+1)); done
  rm -rf "/work/out.$2" "/work/out.$2.ready" "/work/out.$2.fetched"
fi
exit "\$rc"
PAGE_END
}

page_command() { # slot, logname, progress, then the argv (the fetch happens while the run still holds the lock)
  set +e
  run_here "$@"
  rc=$?
  set -e
  return "$rc"
}

# ───────── local mode ─────────

local_cargo() {
  if cargo --version >/dev/null 2>&1; then cargo "$@"; else mise x rust@stable -- cargo "$@"; fi
}

# A page run on this machine, like the one on the yard: the page and the binary are built first.
local_build() {
  sh scripts/web-build.sh && local_cargo build --quiet
}

local_work() {
  target="${HERDR_PLANNER_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/target}"
  CARGO_TARGET_DIR=$target
  export CARGO_TARGET_DIR
  mkdir -p "$LOG_DIR/out/shots"
  case "$1" in
    build) shift; local_cargo build "$@" ;;
    test) shift; local_cargo test "$@" ;;
    web) shift; exec sh scripts/web-build.sh "$@" ;;
    run) shift; exec "$@" ;;
    section)
      names=${2:-}
      [ -n "$names" ] || die 2 "yard: section: назовите секции, например «runners todo»"
      local_build || exit
      SECTIONS=$names exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    pages)
      parts=${2:-}
      local_build || exit
      PAGE_ONLY=1 SHARDS=$parts exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    check) exec sh scripts/run-checks.sh "$LOG_DIR/out/shots" ;;
    quick) HERDR_PLANNER_TARGET_DIR=$target CARGO_TARGET_DIR=$target exec sh -c "$(quick_script)" ;;
    fetch) say "YARD=local: снимки и тексты — $LOG_DIR/out" ;;
    down|up|sync|status|prune) say "YARD=local: контейнера нет, всё на этой машине" ;;
    where) say "YARD=local: верфь не нужна, всё на этой машине" ;;
    *) usage; exit 2 ;;
  esac
}

# ───────── the commands ─────────

cmd=${1:-help}
if [ $# -gt 0 ]; then shift; fi
# What the container's journal calls the command (PLN-56): the wrapper's own word and its arguments.
LABEL=$cmd
if [ $# -gt 0 ]; then LABEL="$cmd $*"; fi

case "$cmd" in
  help|-h|--help) usage; exit 0 ;;
  build|test|web|run|section|pages|check|quick|fetch|down|up|sync|status|prune|where) ;;
  *) usage; exit 2 ;;
esac

mkdir -p "$LOG_DIR"

if [ "${YARD:-}" = local ]; then
  local_work "$cmd" "$@"
  exit $?
fi

TAG=$(task_tag)
NAME="yard-$TAG"
IMAGE=$(image_tag)
resolve_yard
[ "$cmd" = where ] || yard_settings

case "$cmd" in
  where) printf '%s\n' "$YARD_SHOW"; exit 0 ;;
  up) up ;;
  sync) up; sync_tree; say "рабочая копия в $NAME:/work/in" ;;
  run)
    [ $# -gt 0 ] || die 2 "yard: run: нужна команда, например «sh scripts/run-checks.sh»"
    up; sync_tree; run_here 0 run 0 "$@" ;;
  build) up; sync_tree; run_here 0 build 0 cargo build "$@" ;;
  test) up; sync_tree; run_here 0 test 0 cargo test "$@" ;;
  web) up; sync_tree; run_here 0 web 0 sh scripts/web-build.sh "$@" ;;
  section)
    names=${1:-}
    [ -n "$names" ] || die 2 "yard: section: назовите секции, например «runners todo»"
    up; sync_tree
    page_command 0 section 1 env SECTIONS="$names" sh -c "$(page_script 1 "$RUN_ID")" ;;
  pages)
    parts=${1:-}
    up; sync_tree
    page_command 1 pages 1 env PAGE_ONLY=1 SHARDS="$parts" sh -c "$(page_script 1 "$RUN_ID")" ;;
  check)
    up; sync_tree
    page_command 1 check 1 env sh -c "$(page_script 0 "$RUN_ID")" ;;
  quick)
    up; sync_tree
    run_here 0 quick 0 env HERDR_PLANNER_TARGET_DIR=/work/target CARGO_TARGET_DIR=/work/target \
      sh -c "$(quick_script)" ;;
  fetch)
    ensure_yard
    run=${1:-}
    if [ -z "$run" ]; then
      run=$($D exec "$NAME" sh -c 'ls -1dt /work/out.* 2>/dev/null | head -1' 2>/dev/null | sed 's|^/work/out\.||')
    fi
    [ -n "$run" ] || die 1 "yard: fetch: нет ни одного результата прогона"
    fetch_out "$run" ;;
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
    say "контейнер задачи: ЦП ${CPUS:-—}, слотов ${SLOTS:-—}, GPU $([ -n "$GPU_FLAGS" ] && echo да || echo нет), память $MEM"
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
