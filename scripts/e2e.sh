#!/usr/bin/env bash
# The end-to-end suite (web/e2e): real aetherd and archond with a faux engine, driven through
# Chromium. Resolves both binaries, runs Playwright in the host CI slot, then fails if any
# daemon process or temporary root outlived the run.
#
#   scripts/e2e.sh [playwright test arguments...]
#
# AETHER_BIN   the aether binary; default: `cargo build -p aether` of this checkout.
# ARCHOND_BIN  the archon binary; default: built once from a read-only clone of mengsig/archon
#              at E2E_ARCHON_BRANCH's head, cached under $CARGO_TARGET_DIR/e2e-archon by commit.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT=$PWD
TARGET=${CARGO_TARGET_DIR:-$REPO_ROOT/target}
case "$TARGET" in /*) ;; *) TARGET=$REPO_ROOT/$TARGET ;; esac
ARCHON_URL=${E2E_ARCHON_URL:-https://github.com/mengsig/archon}
ARCHON_BRANCH=${E2E_ARCHON_BRANCH:-archon/qryx61z9-vnext-phase-0-baselines-architecture-sha}
ARCHON_CACHE=$TARGET/e2e-archon
SCHEMA=$REPO_ROOT/web/schemas/archon.protocol.schema.json
SCHEMA_IN_ARCHON=crates/archon-core/protocol.schema.json

# Removed on any exit: the run's state dir and a half-done archon clone.
STATE=
CLONE=
trap 'rm -rf -- ${STATE:+"$STATE"} ${CLONE:+"$CLONE"}' EXIT

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31me2e: %s\033[0m\n' "$*" >&2; exit 1; }

# A network step, three tries: GitHub answers 5xx now and then.
retry() {
  local attempt
  for attempt in 1 2 3; do
    if "$@"; then return 0; fi
    [ "$attempt" = 3 ] || { echo "e2e: '$*' failed; retrying in 5s" >&2; sleep 5; }
  done
  return 1
}

resolve_aether() {
  if [ -n "${AETHER_BIN:-}" ]; then return; fi
  say "building aether from this checkout"
  sh scripts/ci_slot.sh cargo build --locked -p aether
  AETHER_BIN=$TARGET/debug/aether
}

schema_sha() { sha256sum "$1" | cut -d' ' -f1; }

# Builds archond at `sha` from a fresh shallow clone, which is deleted afterwards.
build_archond() {
  local sha=$1 binary=$2 src head
  mkdir -p "$ARCHON_CACHE"
  src=$(mktemp -d "$ARCHON_CACHE/src-XXXXXX")
  CLONE=$src
  say "cloning $ARCHON_URL at $ARCHON_BRANCH ($sha), read-only"
  retry git clone --quiet --depth 1 --branch "$ARCHON_BRANCH" "$ARCHON_URL" "$src" || die "could not clone $ARCHON_URL"
  head=$(git -C "$src" rev-parse HEAD)
  [ "$head" = "$sha" ] || die "$ARCHON_BRANCH moved while building ($sha, now $head); run again"
  [ "$(schema_sha "$src/$SCHEMA_IN_ARCHON")" = "$(schema_sha "$SCHEMA")" ] ||
    die "archon $sha's $SCHEMA_IN_ARCHON differs from the vendored web/schemas copy; refresh it (the SYNC errand) or set ARCHOND_BIN"
  say "building archond $sha into $ARCHON_CACHE"
  # From inside the clone, so its rust-toolchain.toml picks the compiler.
  sh scripts/ci_slot.sh sh -c 'cd "$1" && CARGO_TARGET_DIR="$2" cargo build --locked -p archon' sh "$src" "$ARCHON_CACHE/target"
  cp "$ARCHON_CACHE/target/debug/archon" "$binary.tmp"
  cp "$src/$SCHEMA_IN_ARCHON" "$binary.schema.json"
  mv "$binary.tmp" "$binary"
  rm -rf -- "$src"
  CLONE=
}

resolve_archond() {
  if [ -n "${ARCHOND_BIN:-}" ]; then return; fi
  local sha binary
  sha=$(retry git ls-remote "$ARCHON_URL" "refs/heads/$ARCHON_BRANCH" | cut -f1) || true
  [ -n "$sha" ] || die "could not read $ARCHON_BRANCH from $ARCHON_URL; set ARCHOND_BIN to run offline"
  binary=$ARCHON_CACHE/archond-$sha
  if [ -x "$binary" ]; then
    say "reusing archond $sha from $ARCHON_CACHE"
  else
    build_archond "$sha" "$binary"
  fi
  [ "$(schema_sha "$binary.schema.json")" = "$(schema_sha "$SCHEMA")" ] ||
    die "archond $sha was built from a schema other than web/schemas/archon.protocol.schema.json"
  ARCHOND_BIN=$binary
}

# A process's kernel start time: with its pid, what no reused pid matches.
started_at() {
  local stat fields
  stat=$(cat "/proc/$1/stat" 2>/dev/null) || return 1
  read -ra fields <<< "${stat##*) }"
  printf '%s\n' "${fields[19]}"
}

# Every daemon the harness started must be gone and every root it made deleted.
check_leftovers() {
  local leftovers=0 pid started name root daemons=0 roots=0
  if [ -s "$STATE/pids" ]; then
    while read -r pid started name; do
      daemons=$((daemons + 1))
      if [ "$(started_at "$pid" || true)" = "$started" ]; then
        echo "e2e: $name (pid $pid) outlived the run; stopping its process group" >&2
        kill -KILL -- "-$pid" 2>/dev/null || true
        leftovers=1
      fi
    done < "$STATE/pids"
  fi
  if [ -s "$STATE/roots" ]; then
    while read -r root; do
      roots=$((roots + 1))
      if [ -e "$root" ]; then echo "e2e: $root outlived the run" >&2; leftovers=1; fi
    done < "$STATE/roots"
  fi
  [ "$daemons" -gt 0 ] && [ "$roots" -gt 0 ] || { echo "e2e: the harness recorded no daemon or root" >&2; leftovers=1; }
  [ "$leftovers" = 0 ] && echo "e2e: clean teardown: $daemons daemon starts all stopped, $roots stack roots all removed"
  return "$leftovers"
}

resolve_aether
resolve_archond
for binary in "$AETHER_BIN" "$ARCHOND_BIN"; do
  case "$binary" in /*) ;; *) die "binary paths are absolute: $binary" ;; esac
  [ -x "$binary" ] || die "not an executable: $binary"
done
if [ ! -f web/node_modules/.package-lock.json ]; then (cd web && npm ci --no-audit --no-fund); fi

# Short, so the stacks' socket paths stay far below the 108-byte limit.
STATE=$(mktemp -d /tmp/ae-run-XXXXXX)
mkdir -m 700 "$STATE/t" "$STATE/home" "$STATE/xdg"
export E2E_STATE=$STATE E2E_TMP=$STATE/t E2E_USER_HOME=$HOME E2E_OUT=${E2E_OUT:-$REPO_ROOT/web/e2e/.out}
export AETHER_BIN ARCHOND_BIN
export PLAYWRIGHT_BROWSERS_PATH=${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}

say "e2e suite: aether $AETHER_BIN, archond $ARCHOND_BIN"
status=0
# The runner itself gets a private HOME and XDG dirs too; the daemons get their own per stack.
sh scripts/ci_slot.sh env -u AETHER_SOCKET -u ARCHON_SOCKET -u AETHER_TOKEN -u AETHER_REMOTE \
  HOME="$STATE/home" XDG_CONFIG_HOME="$STATE/xdg/config" XDG_DATA_HOME="$STATE/xdg/data" \
  XDG_CACHE_HOME="$STATE/xdg/cache" XDG_STATE_HOME="$STATE/xdg/state" TMPDIR="$STATE/t" \
  node web/node_modules/@playwright/test/cli.js test -c web/e2e/playwright.config.ts "$@" || status=$?
check_leftovers || status=1
exit "$status"
