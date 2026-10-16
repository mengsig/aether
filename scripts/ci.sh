#!/usr/bin/env bash
# Local CI: the same checks a hosted runner would do. Run before pushing.
set -euo pipefail
cd "$(dirname "$0")/.."
# Git exports repository selectors to hooks, especially in linked worktrees.
# Fixture commands must resolve their own -C directory, never initialize or
# commit into the repository whose pre-push hook is running.
while IFS= read -r git_env; do
  unset "$git_env"
done < <(git rev-parse --local-env-vars)
export PATH="$HOME/.cargo/bin:$PATH"
step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }

wait_for_captures() {
  local pid status=0
  for pid in "$@"; do
    if wait "$pid"; then
      continue
    else
      printf 'TUI capture session %s failed\n' "$pid" >&2
      status=1
    fi
  done
  return "$status"
}

# A non-interactive runner shell does not source nvm's profile hook, so
# `node` can be missing even though nvm has it. Fall back to nvm's newest
# installed version rather than depending on runner shell init.
if ! command -v node >/dev/null 2>&1; then
  nvm_dir="${NVM_DIR:-$HOME/.nvm}"
  # Plain `ls` (no GNU `sort -V`, for the macOS job): version dirs are short
  # enough that alphabetic-last picks the newest for any realistic install.
  newest=$(ls -d "$nvm_dir"/versions/node/*/bin 2>/dev/null | tail -1)
  if [ -n "$newest" ]; then
    export PATH="$newest:$PATH"
  fi
fi
AETHER_CI_BINARY="${CARGO_TARGET_DIR:-target}/release/aether"
mkdir -p target

usage() {
  printf 'usage: %s [lint|test|binary]...\n' "$0" >&2
  printf 'Runs the named stages in the order given; all three when none is named.\n' >&2
}

run_lint() {
  step "scripts/lib_test.sh"; scripts/lib_test.sh
  step "sh -n install.sh"; sh -n install.sh
  if command -v shellcheck >/dev/null; then
    step "shellcheck (installer and release scripts)"
    shellcheck install.sh scripts/release.sh scripts/ci_scope.sh
  else
    printf '\n\033[1;33m==> shellcheck of install.sh skipped (needs shellcheck)\033[0m\n'
  fi
  if command -v python3 >/dev/null; then
    step "third-party notices match Cargo.lock"
    python3 scripts/third_party_notices.py --check
    step "links and anchors in the documents"
    python3 scripts/check_links.py
  else
    printf '\n\033[1;33m==> third-party notices and link checks skipped (needs python3)\033[0m\n'
  fi
  step "cargo fmt --check";   cargo fmt --all --check
  step "cargo clippy";        cargo clippy --workspace --all-targets -- -D warnings
}

run_test() {
  step "cargo test";          cargo test --workspace
}

check_client_drift() {
  # The client libraries are generated from the Rust protocol types. A type that
  # changed without a regeneration is a client that lies about the wire, so it
  # fails here rather than at someone else's runtime.
  if command -v python3 >/dev/null; then
    step "client libraries match the protocol types"
    python3 scripts/generate_clients.py --check --binary "$AETHER_CI_BINARY"
  else
    printf '\n\033[1;33m==> client library drift check skipped (needs python3)\033[0m\n'
  fi
}

check_python_suites() {
  # The hand-written halves of each client (handshake, id multiplexing,
  # keepalive, failure settling) are not touched by the drift check above, and
  # nothing else runs them. Both suites spin their own stub daemon in-process.
  if command -v python3 >/dev/null; then
    step "daemon fixture lifecycle tests"
    python3 scripts/test_daemon_lifecycle.py
    step "TUI process measurement harness tests"
    python3 -m unittest discover -s scripts -p test_tui_process_budgets.py
    step "soak driver evidence tests"
    python3 -m unittest discover -s scripts -p test_soak_mock.py
    step "capture checker fixture tests"
    python3 -m unittest discover -s scripts -p test_check_captures.py
    step "TUI capture startup failure tests"
    python3 -m unittest discover -s scripts -p test_tui_capture.py
    step "ANSI screenshot background tests (needs Pillow)"
    python3 -m unittest discover -s scripts -p test_ansi2png.py
    # dash is the strictest /bin/sh in common use; without it the test skips
    # `test_dash_is_one_of_the_shells` and unittest ends with "OK (skipped=1)".
    step "installer tests (install.sh against a local fake release)"
    command -v dash >/dev/null \
      || printf '\033[1;33mdash is not installed: install.sh is tested under sh only\033[0m\n'
    python3 -m unittest discover -s scripts -p test_install.py
    step "release script tests (release notes and archive)"
    python3 -m unittest discover -s scripts -p test_release.py
    step "third-party notices generator tests"
    python3 -m unittest discover -s scripts -p test_third_party_notices.py
    step "release install check tests (a fake binary, the real install.sh)"
    python3 -m unittest discover -s scripts -p test_release_install_check.py
    step "link check tests"
    python3 -m unittest discover -s scripts -p test_check_links.py
    step "upgrade lane tests (release choice, schema rewind, fake engine)"
    python3 -m unittest discover -s scripts -p test_upgrade_compat.py
    step "CI scope tests (which changes skip the Rust checks)"
    python3 -m unittest discover -s scripts -p test_ci_scope.py
    step "client generator layout tests"
    python3 -m unittest discover -s scripts -p 'test_generate_clients_*.py'
    step "python client tests"
    python3 -m unittest discover -s clients/python/tests
  else
    printf '\n\033[1;33m==> python client tests skipped (needs python3)\033[0m\n'
  fi
}

check_typescript_client() {
  if command -v node >/dev/null && command -v npm >/dev/null; then
    step "typescript client: typecheck + tests"
    (cd clients/typescript && npm ci && npm run typecheck && npm test)
  else
    printf '\n\033[1;33m==> typescript client tests skipped (needs node >=22.6 and npm; nvm at ~/.nvm not found)\033[0m\n'
  fi
}

check_remote_web() {
  if command -v node >/dev/null && command -v npm >/dev/null; then
    step "remote web: typecheck + unit tests + available browser checks"
    (
      cd crates/aether-remote/web
      # npm ci removes node_modules, including locally downloaded browsers.
      # Bootstrap fresh checkouts; preserve existing local test dependencies.
      if [ ! -f node_modules/.package-lock.json ]; then
        npm ci --cache node_modules/.npm-cache
      fi
    )
    # The web runner adds npm run test:live -- <binary> with Python and Chromium.
    AETHER_BINARY="$AETHER_CI_BINARY" node crates/aether-remote/web/scripts/ci.js
  else
    printf '\n\033[1;33m==> remote web checks skipped (needs node and npm)\033[0m\n'
  fi
}

check_release_gates() {
  step "cargo test --release (timing)"; cargo test --release -p aether-runtime --test rollup_bench -- --ignored

  # VISION §13 asks for the mock end-to-end here, not only in the hosted
  # workflow: it is the only thing that drives the real binary against a real
  # daemon over a real socket, and the only gate on --plain over a closing pipe.
  if command -v python3 >/dev/null; then
    step "mock end-to-end"
    scripts/e2e_mock.sh "$AETHER_CI_BINARY"
  else
    printf '\n\033[1;33m==> mock end-to-end skipped (needs python3)\033[0m\n'
  fi
  # §9.8's numbers are release numbers. The debug run above skips them, so this
  # is the only place the performance budgets are a gate. Single-threaded: the
  # memory budget measures the process, and the timings measure a busy machine
  # otherwise.
  step "9.8 budgets (release)"
  cargo test --release -p aether-tui --test budgets --test budget_memory \
    -- --nocapture --test-threads=1
  step "9.8 process measurements (release; idle zero/no-timers remains open)"
  cargo test --release -p aether-tui --lib runner::process_tests::measured_process_budgets \
    -- --exact --nocapture --test-threads=1
}

# The release workflow's install check, on the binary this stage built. On Linux
# that binary links glibc; it is packed under the name install.sh asks for here.
check_release_install() {
  if ! command -v python3 >/dev/null; then
    printf '\n\033[1;33m==> release install check skipped (needs python3)\033[0m\n'
    return 0
  fi
  step "release install check (install.sh, then aether update --check, against a local release)"
  local host target tmp_base dist
  host=$(rustc -vV | sed -n 's/^host: //p')
  target=${host/-linux-gnu/-linux-musl}
  tmp_base="${TMPDIR:-/tmp}"
  dist=$(mktemp -d "${tmp_base%/}/aether-ci-dist-XXXXXX")
  scripts/release.sh archive "$target" "$AETHER_CI_BINARY" "$dist"
  scripts/release.sh install-check "$target" "$dist"
  rm -rf "$dist"
}

check_container_backend() {
  # The container backend lane is not part of `cargo test --workspace`: it needs
  # Docker and a local image, and a test that skips itself when they are missing
  # would report green while asserting nothing. Run it deliberately:
  #
  #   AETHER_TEST_CONTAINER_IMAGE=python:3.12-slim \
  #     cargo test -p aether-tools --test container_backend -- --ignored --test-threads=1
  #
  # Set AETHER_TEST_CONTAINER_IMAGE here and this script runs it too.
  if [ -n "${AETHER_TEST_CONTAINER_IMAGE:-}" ]; then
    step "container backend (image: $AETHER_TEST_CONTAINER_IMAGE)"
    # These integration tests share one Docker daemon; avoid overlapping
    # container startups while testing cancellation and orphan cleanup.
    cargo test -p aether-tools --test container_backend -- --ignored --test-threads=1
  else
    printf '\n\033[1;33mnot run: container backend lane\033[0m\n'
    printf 'ARCHITECTURE 12.7 is evidenced by that lane. Set '
    printf 'AETHER_TEST_CONTAINER_IMAGE to a local image with bash to run it.\n'
  fi
}

# Frames from the real terminal, against the real daemon and the mock
# provider. The Rust tests cover what a TestBackend can see; this covers what
# tmux actually painted. Skipped where tmux or python3 is missing.
check_captures() {
  if ! command -v tmux >/dev/null || ! command -v python3 >/dev/null; then
    printf '\n\033[1;33m==> tui captures skipped (needs tmux and python3)\033[0m\n'
    return 0
  fi
  step "tui captures"
  # Strip a trailing slash from TMPDIR (macOS's default) so the template
  # below never doubles up; see scripts/lib.sh's tmp_base for the same fix.
  tmp_base="${TMPDIR:-/tmp}"
  CAPTURES=$(mktemp -d "${tmp_base%/}/aether-ci-captures-XXXXXX")
  captures_passed=0
  trap 'if [ "$captures_passed" = 1 ]; then rm -rf "$CAPTURES"; else printf "Failed captures retained in %s\n" "$CAPTURES"; fi' EXIT
  echo "== lane: wide (160) full sessions =="
  capture_pids=()
  for theme in auto dark light mono; do
    scripts/tui_capture.sh "$AETHER_CI_BINARY" "$CAPTURES/$theme" "$theme" >/dev/null &
    capture_pids+=("$!")
  done
  wait_for_captures "${capture_pids[@]}"
  python3 scripts/check_captures.py "$CAPTURES/dark" --theme dark --width 160 --check-board
  # Auto runs where the terminal never answers the palette probe, so it must
  # still paint, and must not be dark's tinted screen.
  python3 scripts/check_captures.py "$CAPTURES/auto" --theme auto --width 160 \
    --differs-from "$CAPTURES/dark"
  python3 scripts/check_captures.py "$CAPTURES/light" --theme light --width 160 \
    --differs-from "$CAPTURES/dark"
  # Mono must not merely pass its own checks; it must not be dark.
  python3 scripts/check_captures.py "$CAPTURES/mono" --theme mono --width 160 \
    --differs-from "$CAPTURES/dark"
  echo "== lane: responsive breakpoints (59/60/99/100), smoke sessions =="
  # The board docks from 100 columns, overlay below (SPEC §1.2). One theme: the
  # layout does not vary by theme, and 4 themes x 4 widths would be 16 sessions.
  capture_pids=()
  for width in 59 60 99 100; do
    SMOKE=1 scripts/tui_capture.sh "$AETHER_CI_BINARY" "$CAPTURES/width-$width" dark "$width" >/dev/null &
    capture_pids+=("$!")
  done
  wait_for_captures "${capture_pids[@]}"
  for width in 59 60 99 100; do
    python3 scripts/check_captures.py "$CAPTURES/width-$width" --lane smoke --theme dark --width "$width" \
      --check-board
  done
  echo "== lane: declarative view mode (80/120), external fixture =="
  for width in 80 120; do
    MODE_LANE=1 scripts/tui_capture.sh "$AETHER_CI_BINARY" "$CAPTURES/mode-$width" dark "$width" >/dev/null
    python3 scripts/check_captures.py "$CAPTURES/mode-$width" --lane mode --theme dark --width "$width"
  done
  captures_passed=1
}

run_binary() {
  step "cargo build --release (aether binary)"; cargo build --release -p aether
  check_client_drift
  check_python_suites
  check_typescript_client
  check_remote_web
  check_release_gates
  check_release_install
  check_container_backend
  check_captures
}

if [ "$#" -eq 0 ]; then
  set -- lint test binary
fi
for stage in "$@"; do
  case "$stage" in
    lint|test|binary) ;;
    *) usage; exit 2 ;;
  esac
done
for stage in "$@"; do
  "run_$stage"
done
printf '\n\033[1;32mCI passed: %s\033[0m\n' "$*"
