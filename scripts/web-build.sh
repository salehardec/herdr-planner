#!/bin/sh
# Builds the page (web/) into ${XDG_CACHE_HOME:-~/.cache}/herdr-planner/web/dist.
# The work happens in a copy outside the repository: the repository may sit on
# a filesystem (NTFS) where the native esbuild/rollup binaries cannot run.
#   scripts/web-build.sh          build once
#   scripts/web-build.sh --dev    start the Vite dev server from the copy
#   scripts/web-build.sh --test   run the page's unit tests (vitest)
#   scripts/web-build.sh --check  type-check the page (tsc --noEmit)
set -eu
cd "$(dirname "$0")/.."
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/herdr-planner/web"
COPY="$CACHE/src-copy"
DIST="${HERDR_PLANNER_DIST:-$CACHE/dist}"
mkdir -p "$COPY"
# Copy the sources; keep node_modules of the copy between runs.
if command -v rsync >/dev/null 2>&1; then
  rsync -a --delete --exclude node_modules --exclude dist web/ "$COPY/"
else
  (cd web && tar cf - --exclude=node_modules --exclude=dist .) | (cd "$COPY" && tar xf -)
fi
cd "$COPY"
STAMP="node_modules/.lock-stamp"
WANT="$(cksum < package-lock.json)"
if [ ! -d node_modules ] || [ "$(cat "$STAMP" 2>/dev/null || true)" != "$WANT" ]; then
  echo "web-build: npm ci"
  npm ci --no-audit --no-fund --loglevel=error
  printf '%s' "$WANT" > "$STAMP"
fi
case "${1:-}" in
  --dev) exec npx vite ;;
  --test) exec npx vitest run ;;
  --check) exec npx tsc --noEmit ;;
esac
HERDR_PLANNER_DIST="$DIST" npx vite build --logLevel warn
echo "web-build: $DIST"
