#!/usr/bin/env bash
# The desktop journeys against the packaged app: `scripts/e2e_journeys.sh [playwright test args]`.
# JOURNEY_ENGINE=faux (default) or real; docs/e2e-journeys.md. Builds no binary; fails if any
# daemon, sway or app process or temp root outlives the run.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
TARGET=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
case "$TARGET" in /*) ;; *) TARGET=$REPO_ROOT/$TARGET ;; esac
SCHEMA=$REPO_ROOT/web/schemas/archon.protocol.schema.json
PACKAGED=$REPO_ROOT/target/desktop/linux-unpacked/aether-desktop
MIN_FREE_KB=$((10 * 1024 * 1024))

STATE=
trap 'rm -rf -- ${STATE:+"$STATE"}' EXIT

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31me2e-journeys: %s\033[0m\n' "$*" >&2; exit 1; }
schema_sha() { sha256sum "$1" | cut -d' ' -f1; }

# AETHER_BIN, when unset, is an existing debug build; this script never runs cargo.
resolve_aether() {
  if [ -n "${AETHER_BIN:-}" ]; then return; fi
  AETHER_BIN=$TARGET/debug/aether
  [ -x "$AETHER_BIN" ] || die "no $AETHER_BIN; set AETHER_BIN or build it first (scripts/e2e.sh builds it)"
}

# ARCHOND_BIN, when unset, is the newest archond scripts/e2e.sh cached with the vendored schema.
resolve_archond() {
  if [ -n "${ARCHOND_BIN:-}" ]; then return; fi
  local candidate newest=
  for candidate in "$TARGET"/e2e-archon/archond-*; do
    case "$candidate" in *.schema.json | *.tmp) continue ;; esac
    [ -x "$candidate" ] && [ -f "$candidate.schema.json" ] || continue
    [ "$(schema_sha "$candidate.schema.json")" = "$(schema_sha "$SCHEMA")" ] || continue
    if [ -z "$newest" ] || [ "$candidate" -nt "$newest" ]; then newest=$candidate; fi
  done
  [ -n "$newest" ] || die "no cached archond in $TARGET/e2e-archon matches web/schemas; set ARCHOND_BIN or run scripts/e2e.sh once"
  ARCHOND_BIN=$newest
}

# The packaged app: E2E_DESKTOP_EXEC, else target/desktop, packaged under the CI slot unless
# it is there already (E2E_REPACKAGE=1 packages again).
resolve_desktop() {
  if [ -n "${E2E_DESKTOP_EXEC:-}" ]; then return; fi
  E2E_DESKTOP_EXEC=$PACKAGED
  if [ -x "$PACKAGED" ] && [ "${E2E_REPACKAGE:-0}" != 1 ]; then
    say "reusing the packaged app at $PACKAGED"
    return
  fi
  local free
  free=$(df -Pk "$REPO_ROOT" | awk 'NR == 2 { print $4 }')
  [ "$free" -gt "$MIN_FREE_KB" ] || die "packaging needs more than 10 GB free under $REPO_ROOT (${free} KB)"
  say "packaging the desktop app (linux, --dir)"
  sh scripts/ci_slot.sh sh -c 'cd web/apps/desktop && node build/package.ts --target linux --dir'
  [ -x "$PACKAGED" ] || die "packaging made no $PACKAGED"
}

started_at() {
  local stat fields
  stat=$(cat "/proc/$1/stat" 2>/dev/null) || return 1
  read -ra fields <<< "${stat##*) }"
  printf '%s\n' "${fields[19]}"
}

# As scripts/e2e.sh: every process the harness recorded (daemons, sway, the app) must be gone
# and every root it made deleted.
check_leftovers() {
  local leftovers=0 pid started name root processes=0 roots=0
  if [ -s "$STATE/pids" ]; then
    while read -r pid started name; do
      processes=$((processes + 1))
      if [ "$(started_at "$pid" || true)" = "$started" ]; then
        echo "e2e-journeys: $name (pid $pid) outlived the run; stopping its process group" >&2
        kill -KILL -- "-$pid" 2>/dev/null || true
        leftovers=1
      fi
    done < "$STATE/pids"
  fi
  if [ -s "$STATE/roots" ]; then
    while read -r root; do
      roots=$((roots + 1))
      if [ -e "$root" ]; then echo "e2e-journeys: $root outlived the run" >&2; leftovers=1; fi
    done < "$STATE/roots"
  fi
  if [ "$LISTING" = 0 ] && { [ "$processes" = 0 ] || [ "$roots" = 0 ]; }; then
    echo "e2e-journeys: the harness recorded no process or root, so nothing was checked" >&2
    leftovers=1
  fi
  [ "$leftovers" = 0 ] && echo "e2e-journeys: clean teardown: $processes process starts all stopped, $roots temp roots all removed"
  return "$leftovers"
}

LISTING=0
for argument in "$@"; do [ "$argument" = --list ] && LISTING=1; done
JOURNEY_ENGINE=${JOURNEY_ENGINE:-faux}
case "$JOURNEY_ENGINE" in faux | real) ;; *) die "JOURNEY_ENGINE is faux or real, not $JOURNEY_ENGINE" ;; esac
resolve_aether
resolve_archond
resolve_desktop
for binary in "$AETHER_BIN" "$ARCHOND_BIN" "$E2E_DESKTOP_EXEC"; do
  case "$binary" in /*) ;; *) die "binary paths are absolute: $binary" ;; esac
  [ -x "$binary" ] || die "not an executable: $binary"
done
for tool in sway swaymsg wtype; do command -v "$tool" > /dev/null || die "the journeys need $tool on PATH"; done
[ -f web/node_modules/.package-lock.json ] || die "web/node_modules is missing; install it (npm ci in web) first"

STATE=$(mktemp -d "${E2E_RUN_BASE:-/var/tmp}/ae-run-XXXXXX")
mkdir -m 700 "$STATE/t" "$STATE/home" "$STATE/xdg"
export E2E_STATE=$STATE E2E_TMP=$STATE/t E2E_USER_HOME=$HOME E2E_USER_PATH=${E2E_USER_PATH:-$PATH} E2E_OUT=${E2E_OUT:-$REPO_ROOT/web/e2e/.out}
export AETHER_BIN ARCHOND_BIN E2E_DESKTOP_EXEC JOURNEY_ENGINE
export PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}

say "journeys ($JOURNEY_ENGINE): aether $AETHER_BIN, archond $ARCHOND_BIN, app $E2E_DESKTOP_EXEC"
status=0
# The runner gets a private HOME and XDG dirs; each stack, sway and app instance gets its own.
sh scripts/ci_slot.sh env -u AETHER_SOCKET -u ARCHON_SOCKET -u AETHER_TOKEN -u AETHER_REMOTE \
  -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK \
  HOME="$STATE/home" XDG_CONFIG_HOME="$STATE/xdg/config" XDG_DATA_HOME="$STATE/xdg/data" \
  XDG_CACHE_HOME="$STATE/xdg/cache" XDG_STATE_HOME="$STATE/xdg/state" TMPDIR="$STATE/t" \
  node web/node_modules/@playwright/test/cli.js test -c web/e2e/specs/journeys/playwright.config.ts "$@" || status=$?
check_leftovers || status=1
exit "$status"
