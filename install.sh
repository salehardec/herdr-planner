#!/bin/sh
# Herdr Planner, the one-line installer entry point.
#
#   curl -fsSL https://github.com/salehardec/herdr-planner/releases/latest/download/install.sh | sh
#   sh install.sh [flags]                  from a clone: runs scripts/install.sh
#
# Without a clone it fetches the release's install.sh (the same file as scripts/install.sh,
# which downloads and checks the release archive itself) and runs that. Flags are passed on.
set -eu

main() {
  if [ -f "$0" ] && [ -f "$(dirname "$0")/scripts/install.sh" ]; then
    exec sh "$(dirname "$0")/scripts/install.sh" "$@"
  fi
  base=${HERDR_PLANNER_RELEASES:-https://github.com/salehardec/herdr-planner/releases}
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/herdr-planner-install.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  url=$base/latest/download/install.sh
  curl -fsSL -o "$tmp/install.sh" "$url" || { echo "install.sh: не скачать $url" >&2; exit 1; }
  # Piped from curl, stdin is the script itself: the installer asks its questions on the terminal.
  if [ -t 0 ]; then sh "$tmp/install.sh" "$@"; else sh "$tmp/install.sh" "$@" </dev/null; fi
}

main "$@"
