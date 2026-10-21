#!/bin/sh
# Proof of stability: the page checks N times in a row, with the machine kept busy on purpose.
#   scripts/stress-checks.sh [runs=10] [burners=nproc]
#   STRESS_NICE=10 makes the checks lower in priority than the busy loops (they get the cores only when they are left).
# Needs the binary and the page already built (run scripts/run-checks.sh once first). Runs retries off
# (NO_RETRY=1), so a check that only passes on its second go still counts as a failure here.
# Prints one line per run and the list of checks that failed or were retried.
set -u
cd "$(dirname "$0")/.."
RUNS="${1:-10}"
BURNERS="${2:-$(nproc 2>/dev/null || echo 4)}"
LOGS="$(mktemp -d)"
BUSY=
trap '[ -z "$BUSY" ] || kill $BUSY 2>/dev/null || true' EXIT INT TERM
# CPU burners at normal priority: the checks have to share the cores with them.
i=0
while [ "$i" -lt "$BURNERS" ]; do
  sh -c 'while :; do :; done' &
  BUSY="$BUSY $!"
  i=$((i + 1))
done
echo "stress: $RUNS runs of the page checks next to $BURNERS busy loops; logs in $LOGS"
bad=0
n=1
while [ "$n" -le "$RUNS" ]; do
  start=$(date +%s)
  if NO_RETRY=1 PAGE_ONLY=1 nice -n "${STRESS_NICE:-0}" sh scripts/run-checks.sh >"$LOGS/run$n.txt" 2>&1; then r=ok; else r=FAILED; bad=$((bad + 1)); fi
  echo "run $n: $r in $(($(date +%s) - start)) s"
  [ "$r" = ok ] || grep -a "^FAIL\|^run-checks:\|^eval \|^RETRY" "$LOGS/run$n.txt" | head -40
  n=$((n + 1))
done
echo "stress: $((RUNS - bad)) of $RUNS runs without a failure"
grep -h "^FAIL\|^RETRY\|^   retried" "$LOGS"/run*.txt | sort | uniq -c | sort -rn
[ "$bad" -eq 0 ]
