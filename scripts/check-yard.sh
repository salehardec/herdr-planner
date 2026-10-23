#!/bin/sh
# Checks scripts/yard.sh without a shipyard: stand-ins for docker, rsync and cargo record every call, and the wrapper is
# run the way an agent, the board and the human run it. What is checked: the task's tag from TMPDIR / the board's id, the
# container's name, labels and limits (no published ports, no bind mounts, the target inside /work), the image tag from
# the Dockerfile's hash, the rsync arguments (--exclude .git, never -a or -t), the exit code passed through, the progress
# file of the board, YARD=local, and «верфь не отвечает» with code 3.
#   scripts/check-yard.sh
set -eu
cd "$(dirname "$0")/.."
REPO=$(pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT INT TERM
ST=$T/state
export ST
BIN=$T/bin
mkdir -p "$ST" "$BIN" "$T/logs" "$T/home/.herdr-projects/settings"
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }

expect() { # description, pattern, file
  if grep -qE -- "$2" "$3" 2>/dev/null; then ok "$1"; else bad "$1: /$2/ не найдено в $3"; sed -n "1,6p" "$3" 2>/dev/null; fi
}
refuse() { # description, pattern, file
  if grep -qE -- "$2" "$3" 2>/dev/null; then bad "$1: /$2/ не должно быть в $3"; else ok "$1"; fi
}
after() { # description, the line that must come first, the line that must come later, file
  if awk -v a="$2" -v b="$3" 'index($0, a) { seen = 1 } index($0, b) { if (!seen) exit 1; hit = 1 } END { exit hit ? 0 : 1 }' "$4" 2>/dev/null; then
    ok "$1"
  else
    bad "$1: /$3/ не идёт после /$2/ в $4"
  fi
}

cat >"$BIN/docker" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$ST/docker.log"
case "$1" in
  version) [ -e "$ST/down" ] && exit 1; echo 28.3.0; exit 0 ;;
  image)
    case "$2" in
      inspect)
        if [ "${3:-}" = "-f" ]; then echo "sha256:aaaaaaaaaaaaaaaa"; exit 0; fi
        [ -e "$ST/image" ] && exit 0 || exit 1 ;;
      ls) [ -e "$ST/stale-image" ] && echo "herdr-planner-yard:000000000000"; exit 0 ;;
    esac
    exit 0 ;;
  build) : >"$ST/image"; exit 0 ;;
  volume)
    case "$2" in
      inspect) [ -e "$ST/volume" ] && exit 0 || exit 1 ;;
      create) : >"$ST/volume"; exit 0 ;;
      rm) : ;;
    esac
    exit 0 ;;
  container)
    case "$2" in inspect) [ -e "$ST/container" ] && exit 0 || exit 1 ;; esac
    exit 0 ;;
  inspect)
    case "$2" in
      '{{.Image}}') echo "sha256:aaaaaaaaaaaaaaaa" ;;
      '{{.State.Running}}') echo true ;;
      *'herdr.yard.cpus'*) cat "$ST/label-cpus" 2>/dev/null || true ;;
      *'herdr.yard.gpu'*) cat "$ST/label-gpu" 2>/dev/null || true ;;
      *) echo running ;;
    esac
    exit 0 ;;
  run)
    : >"$ST/container"
    for a in "$@"; do
      case "$a" in
        herdr.yard.cpus=*) printf '%s' "${a#*=}" >"$ST/label-cpus" ;;
        herdr.yard.gpu=*) printf '%s' "${a#*=}" >"$ST/label-gpu" ;;
      esac
    done
    echo container-id; exit 0 ;;
  start) : >"$ST/container"; exit 0 ;;
  rm) rm -f "$ST/container"; exit 0 ;;
  exec)
    case "$*" in
      *"cat /work/progress."*) cat "$ST/progress" 2>/dev/null; exit 0 ;;
      *"test -d /work/out"*) [ -d "$ST/out" ] && exit 0 || exit 1 ;;
    esac
    exit "${YARD_STUB_RC:-0}" ;;
  cp) exit 0 ;;
  ps) exit 0 ;;
esac
exit 0
EOF
cat >"$BIN/rsync" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$ST/rsync.log"
exit 0
EOF
cat >"$BIN/cargo" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$ST/cargo.log"
exit 0
EOF
chmod +x "$BIN/docker" "$BIN/rsync" "$BIN/cargo"

fresh() {
  rm -f "$ST"/docker.log "$ST"/rsync.log "$ST"/cargo.log "$ST"/container "$ST"/image "$ST"/volume \
    "$ST"/down "$ST"/progress "$ST"/stale-image "$ST"/label-cpus "$ST"/label-gpu
  rm -rf "$ST/out"
  : >"$ST/docker.log"; : >"$ST/rsync.log"; : >"$ST/cargo.log"
}

# Runs the wrapper as an agent/board/human would; sets $status and $out.
yard() { # TMPDIR, then the arguments
  tmp=$1
  shift
  set +e
  env PATH="$BIN:$PATH" HOME="$T/home" TMPDIR="$tmp" DOCKER_HOST="ssh://vm-r" \
    HERDR_PROJECTS_ID= HERDR_PROJECTS_TAG= YARD= YARD_STUB_RC="${YARD_STUB_RC:-0}" \
    HERDR_PROJECTS_PROGRESS="${HERDR_PROJECTS_PROGRESS:-}" \
    HERDR_YARD_LOG_DIR="$T/logs" /bin/sh "$REPO/scripts/yard.sh" "$@" >"$T/out" 2>&1
  status=$?
  set -e
}

HASH=$( (sha256sum scripts/yard/Dockerfile 2>/dev/null || shasum -a 256 scripts/yard/Dockerfile) | cut -c1-12)

# ── the tag from TMPDIR, the image, the container ──
fresh
mkdir -p "$T/pln/PLN-7"
yard "$T/pln/PLN-7" up
[ "$status" = 0 ] || bad "up: код $status ($(cat "$T/out"))"
expect "образ собран из Dockerfile, тег — его хеш" "build .* -t herdr-planner-yard:$HASH" "$ST/docker.log"
expect "образ помечен herdr.shared=herdr-planner" "build --label herdr.shared=herdr-planner" "$ST/docker.log"
expect "том кэша создан с той же меткой" "volume create --label herdr.shared=herdr-planner hp-yard-cache" "$ST/docker.log"
expect "контейнер задачи назван по TMPDIR и помечен herdr.task" "run -d --name yard-PLN-7 --label herdr.task=PLN-7" "$ST/docker.log"
expect "лимиты контейнера: 6 ЦП, 12 ГБ, 4096 процессов, no-new-privileges" "--cpus 6 --memory 12g --pids-limit 4096 --security-opt no-new-privileges" "$ST/docker.log"
expect "настройки верфи записаны в метки контейнера" "--label herdr.yard.cpus=6 --label herdr.yard.gpu=0" "$ST/docker.log"
expect "том кэша смонтирован в /cache" "-v hp-yard-cache:/cache" "$ST/docker.log"
expect "target сборки — внутри /work, не в общем кэше" "-e CARGO_TARGET_DIR=/work/target" "$ST/docker.log"
refuse "ни одного опубликованного порта" " -p [0-9]" "$ST/docker.log"
refuse "ни одного bind-mount хостового пути" " -v /" "$ST/docker.log"
refuse "без --privileged и без --network host" "--privileged|--network host" "$ST/docker.log"

# The image and the volume are not built twice; a started container is not made again.
: >"$ST/docker.log"
yard "$T/pln/PLN-7" up
expect "образ не собирается, когда он есть" "image inspect" "$ST/docker.log"
refuse "повторный up не собирает образ" "build --label" "$ST/docker.log"

# ── the working copy, the run, the exit code ──
fresh
yard "$T/pln/PLN-7" test memory
[ "$status" = 0 ] || bad "test: код $status ($(cat "$T/out"))"
expect "rsync копирует в контейнер" "yard-PLN-7:/work/in/" "$ST/rsync.log"
expect "rsync считает содержимое, а не время" "^-rlc " "$ST/rsync.log"
expect "rsync берёт .gitignore, а .git исключает руками" "--exclude .git --filter=:- .gitignore" "$ST/rsync.log"
refuse "rsync без -a и без -t" "(^| )-(a|t)( |$)" "$ST/rsync.log"
expect "замок на всю команду и копию под ним" "flock /work/.lock sh -c" "$ST/docker.log"
expect "cargo test с аргументом" "cargo test memory" "$ST/docker.log"
refuse "сборки и тесты не занимают слот полного прогона" "YARD_SLOT_USE=1" "$ST/docker.log"

fresh
YARD_STUB_RC=7
yard "$T/pln/PLN-7" test
[ "$status" = 7 ] || bad "код выхода команды доходит до агента (получено $status, ждали 7)"
YARD_STUB_RC=0

# ── the board's id and the repository's key ──
printf '{"repos":{"alpha":{"repo":{"key":"ALP"}}}}' >"$T/home/.herdr-projects/settings/config.json"
fresh
mkdir -p "$T/none"
set +e
env PATH="$BIN:$PATH" HOME="$T/home" TMPDIR="$T/none" DOCKER_HOST="ssh://vm-r" \
  HERDR_PROJECTS_ID=alpha/7 HERDR_PROJECTS_TAG= YARD= YARD_STUB_RC=0 HERDR_YARD_LOG_DIR="$T/logs" \
  /bin/sh "$REPO/scripts/yard.sh" up >"$T/out" 2>&1
status=$?
set -e
[ "$status" = 0 ] || bad "up с HERDR_PROJECTS_ID: код $status ($(cat "$T/out"))"
expect "тег из HERDR_PROJECTS_ID и ключа репозитория" "run -d --name yard-ALP-7 --label herdr.task=ALP-7" "$ST/docker.log"
rm -f "$T/home/.herdr-projects/settings/config.json"

# ── the board's progress file, the slots and the run's own folder ──
fresh
printf '3/11 parts\n' >"$ST/progress"
mkdir -p "$ST/out"
HERDR_PROJECTS_PROGRESS="$T/progress"
yard "$T/pln/PLN-7" pages ui2
[ "$status" = 0 ] || bad "pages: код $status ($(cat "$T/out"))"
expect "полный прогон берёт слот" "YARD_SLOT_USE=1" "$ST/docker.log"
expect "прогресс доски переписан из контейнера" "^3/11 parts$" "$T/progress"
expect "путь прогресса прокинут всему exec — семафор тоже его видит" "-e HERDR_PROJECTS_PROGRESS=/work/progress\." "$ST/docker.log"
refuse "прогресс — свой у каждого прогона, не общий /work/out/progress" "/work/out/progress" "$ST/docker.log"
expect "страница собирается перед показом части" "sh scripts/web-build.sh" "$ST/docker.log"
expect "и бинарник тоже" "cargo build --quiet" "$ST/docker.log"
expect "/work/out готовится внутри команды" "^rm -rf /work/out " "$ST/docker.log"
expect "и старые папки прогонов убираются там же" "/work/out\\.\\*" "$ST/docker.log"
after "очистка /work/out идёт только под замком" "flock /work/.lock" "rm -rf /work/out" "$ST/docker.log"
refuse "нет отдельной очистки /work/out вне замка" "sh -c rm -rf /work/out && mkdir" "$ST/docker.log"
expect "результат прогона уезжает в свою папку" "mv /work/out \"/work/out\." "$ST/docker.log"
expect "прогон держит замок, пока папку не заберут: ждёт .fetched" "while \[ ! -e \"/work/out\..*\.fetched\"" "$ST/docker.log"
expect "снимки забираются из своей папки, а не из общей" "cp yard-PLN-7:/work/out\." "$ST/docker.log"
after "ждём готовности папки раньше копирования" ".ready" "cp yard-PLN-7:/work/out." "$ST/docker.log"
after "копия — раньше, чем отпускаем прогон" "cp yard-PLN-7:/work/out." "touch /work/out." "$ST/docker.log"

fresh
HERDR_PROJECTS_PROGRESS="$T/progress2"
yard "$T/pln/PLN-7" check
[ "$status" = 0 ] || bad "check: код $status ($(cat "$T/out"))"
expect "check идёт по всему run-checks.sh со снимками в /work/out" "sh scripts/run-checks.sh /work/out/shots" "$ST/docker.log"
expect "check тоже готовит /work/out под замком" "^mkdir -p /work/out$" "$ST/docker.log"

fresh
yard "$T/pln/PLN-7" section runners
[ "$status" = 0 ] || bad "section: код $status ($(cat "$T/out"))"
expect "section передаёт секции" "SECTIONS=runners" "$ST/docker.log"
expect "section собирает страницу и бинарник" "sh scripts/web-build.sh" "$ST/docker.log"
refuse "section не занимает слот полного прогона" "YARD_SLOT_USE=1" "$ST/docker.log"

# ── a run waiting for a slot says so (the INNER loop, on a rewritten copy of its paths) ──
fresh
ROOT=$T/inner
mkdir -p "$ROOT/work/src" "$ROOT/work/in" "$ROOT/work/out" "$ROOT/cache/slots"
awk '/^INNER=\$\(cat <<.INNER_EOF.$/ { f = 1; next } /^INNER_EOF$/ { f = 0 } f' scripts/yard.sh |
  sed "s|/work|$ROOT/work|g; s|/cache|$ROOT/cache|g" >"$T/inner.sh"
( flock 9; sleep 20 ) 9>"$ROOT/cache/slots/1" &
HOLD=$!
sleep 0.5
set +e
env HERDR_PROJECTS_PROGRESS="$T/inner-progress" YARD_SLOT_USE=1 YARD_SLOTS=1 timeout 5 sh "$T/inner.sh" yard true >/dev/null 2>&1
set -e
kill "$HOLD" 2>/dev/null || true
wait "$HOLD" 2>/dev/null || true
expect "ожидающий слот пишет состояние ожидания" '"state":"waiting"' "$T/inner-progress"

# ── down keeps the cache ──
fresh
yard "$T/pln/PLN-7" up
: >"$ST/docker.log"
yard "$T/pln/PLN-7" down
expect "down убирает контейнер по точному имени" "rm -f -v yard-PLN-7" "$ST/docker.log"
refuse "down не трогает том кэша" "volume rm" "$ST/docker.log"

# ── a yard that does not answer ──
fresh
: >"$ST/down"
yard "$T/pln/PLN-7" check
[ "$status" = 3 ] || bad "молчащая верфь: код $status (ждали 3)"
expect "и слова о том, что верфь не отвечает" "верфь vm-r не отвечает" "$T/out"
refuse "и никакой сборки на этой машине" "cargo|run-checks" "$T/out"
rm -f "$ST/down"

# ── YARD=local: the same commands here, no container ──
fresh
set +e
env PATH="$BIN:$PATH" HOME="$T/home" TMPDIR="$T/pln/PLN-7" DOCKER_HOST= HERDR_PROJECTS_ID= \
  HERDR_PROJECTS_TAG= YARD=local HERDR_YARD_LOG_DIR="$T/logs" \
  /bin/sh "$REPO/scripts/yard.sh" build --release >"$T/out" 2>&1
status=$?
set -e
[ "$status" = 0 ] || bad "YARD=local build: код $status ($(cat "$T/out"))"
expect "YARD=local зовёт cargo на этой машине" "^build --release$" "$ST/cargo.log"
refuse "YARD=local не говорит с docker" "run -d" "$ST/docker.log"

# ── prune: stale images, only with -y ──
fresh
: >"$ST/stale-image"
: >"$ST/image"
yard "$T/pln/PLN-7" prune
refuse "prune без -y ничего не удаляет" "image rm" "$ST/docker.log"
expect "prune показывает старый образ" "herdr-planner-yard:000000000000" "$T/out"
fresh
: >"$ST/stale-image"
: >"$ST/image"
yard "$T/pln/PLN-7" prune -y
expect "prune -y убирает образ не текущего хеша" "image rm herdr-planner-yard:000000000000" "$ST/docker.log"

# ── several yards: the task's or the repository's rule, tags, a busy or silent yard, the next one ──
cat >"$BIN/ssh" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$ST/ssh.log"
host=
for a in "$@"; do
  case "$a" in vm-r|vm-g) host=$a ;; esac
done
[ -n "$host" ] || exit 255
[ -e "$ST/down-$host" ] && exit 255
case "$*" in
  *nvidia-smi*) [ -e "$ST/no-gpu" ] && exit 1; echo 'GPU 0: NVIDIA L4 (UUID: GPU-00000000)'; exit 0 ;;
  *"/dev/dri"*) [ -e "$ST/no-dri" ] && exit 1; exit 0 ;;
esac
printf '%s 8\n' "$(cat "$ST/load-$host" 2>/dev/null || echo 0.10)"
EOF
chmod +x "$BIN/ssh"
YFILE=$T/home/.herdr-projects/settings/runners.json
write_yards() {
  cat >"$YFILE" <<'EOF'
{
  "version": 1,
  "default": "vm-r",
  "busy_pct": 85,
  "runners": [
    { "id": "vm-r", "target": "vm-r", "tags": ["x86_64"] },
    { "id": "vm-g", "target": "vm-g", "tags": ["gpu", "x86_64"] }
  ],
  "assign": {},
  "need": { "alpha": ["gpu"] },
  "tasks": {}
}
EOF
}
write_yards
printf '{"repos":{"alpha":{"repo":{"key":"ALP"}}}}' >"$T/home/.herdr-projects/settings/config.json"
rm -f "$ST/down-vm-r" "$ST/down-vm-g"
printf '0.10' >"$ST/load-vm-r"; printf '0.20' >"$ST/load-vm-g"
yard_pick() { # TMPDIR, then the arguments
  tmp=$1
  shift
  set +e
  env PATH="$BIN:$PATH" HOME="$T/home" TMPDIR="$tmp" DOCKER_HOST= HERDR_PROJECTS_ID= HERDR_PROJECTS_TAG= YARD= \
    YARD_RUNNER="${YARD_RUNNER:-}" HERDR_YARD_LOG_DIR="$T/logs" /bin/sh "$REPO/scripts/yard.sh" "$@" >"$T/out" 2>"$T/err"
  status=$?
  set -e
}
mkdir -p "$T/pln/ALP-7"
yard_pick "$T/pln/ALP-7" where
[ "$status" = 0 ] || bad "where: код $status ($(cat "$T/out"))"
if [ "$(cat "$T/out")" = "vm-g" ]; then ok "the repository's tags choose the yard that carries them"; else bad "метки репозитория: $(cat "$T/out")"; fi
# A task's own rule replaces the repository's, tags and all.
jq '.tasks = {"alpha/7": {"runner": "vm-r"}}' "$YFILE" >"$YFILE.new" && mv "$YFILE.new" "$YFILE"
yard_pick "$T/pln/ALP-7" where
if [ "$(cat "$T/out")" = "vm-r" ]; then ok "the task's own yard wins over the repository's tags"; else bad "правило задачи: $(cat "$T/out")"; fi
# No rule: the default; a busy default gives way to the next one the list allows.
write_yards
printf '7.50' >"$ST/load-vm-r"
yard_pick "$T/pln/PLN-7" where
if [ "$(cat "$T/out")" = "vm-g" ]; then ok "a busy default yard gives way to the next"; else bad "занятая верфь: $(cat "$T/out")"; fi
yard_pick "$T/pln/PLN-7" where
expect "and the busy one was really checked over ssh" "vm-r" "$ST/ssh.log"
# A list without busy_pct (the real file now) means 85 %, as in the planner, not «never busy».
jq 'del(.busy_pct) | .runners |= map(del(.busy_pct))' "$YFILE" >"$YFILE.new" && mv "$YFILE.new" "$YFILE"
printf '7.50' >"$ST/load-vm-r"
yard_pick "$T/pln/PLN-7" where
if [ "$(cat "$T/out")" = "vm-g" ]; then ok "a list without busy_pct uses the 85 % threshold"; else bad "порог по умолчанию: $(cat "$T/out")"; fi
write_yards
# All busy: the least loaded one is still named.
printf '7.20' >"$ST/load-vm-g"
yard_pick "$T/pln/PLN-7" where
if [ "$(cat "$T/out")" = "vm-g" ]; then ok "all busy: the least loaded is taken"; else bad "все заняты: $(cat "$T/out")"; fi
expect "and the wrapper says so" "наименее загруженную" "$T/err"
# A silent yard is skipped; with none answering the wrapper stops with code 3 and local work is not run.
printf '0.10' >"$ST/load-vm-r"
: >"$ST/down-vm-g"
yard_pick "$T/pln/PLN-7" where
if [ "$status" = 0 ] && [ "$(cat "$T/out")" = "vm-r" ]; then ok "a silent yard is skipped for one that answers"; else bad "молчащая верфь: код $status ($(cat "$T/out"))"; fi
: >"$ST/down-vm-r"
yard_pick "$T/pln/PLN-7" where
[ "$status" = 3 ] || bad "ни одна верфь не отвечает: код $status (ждали 3)"
expect "and no yard is named" "не отвечает" "$T/err"
rm -f "$ST/down-vm-r" "$ST/down-vm-g"
# YARD_RUNNER names one outright.
YARD_RUNNER=vm-r yard_pick "$T/pln/PLN-7" where
if [ "$(cat "$T/out")" = "vm-r" ]; then ok "YARD_RUNNER names the yard outright"; else bad "YARD_RUNNER: $(cat "$T/out")"; fi
YARD_RUNNER=

# ── the GPU and the check container's settings (PLN-58) ──
printf '0.10' >"$ST/load-vm-g"; printf '0.10' >"$ST/load-vm-r"
rm -f "$ST/no-gpu" "$ST/no-dri" "$ST/down-vm-g" "$ST/down-vm-r"
cat >"$YFILE" <<'EOF'
{
  "version": 1,
  "default": "vm-g",
  "runners": [
    { "id": "vm-g", "target": "vm-g", "tags": ["docker28", "gpu"], "check_cpus": 4, "check_slots": 1 },
    { "id": "vm-r", "target": "vm-r", "tags": ["x86_64"] }
  ]
}
EOF
fresh
yard_pick "$T/pln/PLN-7" up
[ "$status" = 0 ] || bad "up на верфи с картой: код $status ($(cat "$T/out"))"
expect "метка gpu и отвечающий nvidia-smi дают контейнеру карту" "--gpus all -e NVIDIA_VISIBLE_DEVICES=all -e NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics" "$ST/docker.log"
expect "и узлы дисплея, когда они есть" "\-\-device /dev/dri" "$ST/docker.log"
expect "ЦП контейнера проверок — из настроек верфи" " --cpus 4 " "$ST/docker.log"
expect "карта и ЦП записаны в метки контейнера" "--label herdr.yard.cpus=4 --label herdr.yard.gpu=1" "$ST/docker.log"

fresh
yard_pick "$T/pln/PLN-7" check
[ "$status" = 0 ] || bad "check на верфи с картой: код $status ($(cat "$T/out"))"
expect "полная проверка берёт слот верфи" "YARD_SLOTS=1" "$ST/docker.log"
expect "очередь полных проверок включена по умолчанию" "YARD_SLOT_USE=1" "$ST/docker.log"

# A yard without the label gets no card, and keeps the defaults.
fresh
YARD_RUNNER=vm-r yard_pick "$T/pln/PLN-7" up
YARD_RUNNER=
[ "$status" = 0 ] || bad "up на верфи без метки gpu: код $status ($(cat "$T/out"))"
refuse "без метки gpu — без --gpus" "\-\-gpus" "$ST/docker.log"
refuse "и без узлов дисплея" "\-\-device /dev/dri" "$ST/docker.log"
expect "по умолчанию 6 ЦП" " --cpus 6 " "$ST/docker.log"

# A label is a promise, the driver is the proof.
: >"$ST/no-gpu"
fresh
yard_pick "$T/pln/PLN-7" up
refuse "метка gpu без отвечающего драйвера — без --gpus" "\-\-gpus" "$ST/docker.log"
rm -f "$ST/no-gpu"

# The card without the display nodes: the container gets the card and no /dev/dri.
: >"$ST/no-dri"
fresh
yard_pick "$T/pln/PLN-7" up
expect "карта и без /dev/dri остаётся в контейнере" "\-\-gpus all" "$ST/docker.log"
refuse "но узлы дисплея не пробрасываются" "\-\-device /dev/dri" "$ST/docker.log"
rm -f "$ST/no-dri"

# 0 CPUs is the human's «no limit»: docker gets no --cpus at all.
jq '.runners[0].check_cpus = 0' "$YFILE" >"$YFILE.new" && mv "$YFILE.new" "$YFILE"
fresh
yard_pick "$T/pln/PLN-7" up
refuse "ЦП 0 — контейнер без --cpus" " --cpus " "$ST/docker.log"
expect "…память и прочее на месте" "--memory 12g" "$ST/docker.log"
expect "…и это видно в метке" "--label herdr.yard.cpus=0" "$ST/docker.log"

# The browser's flags: the card when there is one, the software renderer otherwise (CI).
if command -v node >/dev/null 2>&1; then
  flags=$(node --input-type=module -e '
    import { chooseMode, gpuFlags } from "./scripts/chrome-gpu.mjs";
    const soft = chooseMode({ UI_GPU: "" }, false);
    const hard = chooseMode({ UI_GPU: "" }, true);
    const forced = chooseMode({ UI_GPU: "swiftshader" }, true);
    console.log(soft, gpuFlags(soft).includes("--disable-gpu"), hard, gpuFlags(hard).includes("--disable-gpu"), forced);
  ' 2>&1) || bad "chrome-gpu.mjs не загрузился: $flags"
  if [ "$flags" = "swiftshader true vulkan false swiftshader" ]; then
    ok "флаги Chrome: без карты — swiftshader, с картой — vulkan, UI_GPU сильнее"
  else
    bad "флаги Chrome: $flags"
  fi
else
  echo "skip флаги Chrome: нет node"
fi

if command -v shellcheck >/dev/null 2>&1 && shellcheck --version >/dev/null 2>&1; then
  if shellcheck -s sh scripts/yard.sh scripts/check-yard.sh; then ok "shellcheck"; else bad "shellcheck"; fi
fi

if [ "$fails" -ne 0 ]; then
  echo "check-yard: $fails failed"
  exit 1
fi
echo "check-yard: ok"
