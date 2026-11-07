#!/bin/sh
# Exercise host CI slots against a private runtime directory.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
slot="$root/scripts/ci_slot.sh"
if ! command -v flock >/dev/null 2>&1 || ! command -v timeout >/dev/null 2>&1; then
    echo "SKIP: ci_slot_test.sh needs flock and timeout"
    exit 0
fi
if [ "${CI_SLOT_TEST_BOUNDED:-}" != 1 ]; then
    CI_SLOT_TEST_BOUNDED=1 export CI_SLOT_TEST_BOUNDED
    exec timeout 60 sh "$0" "$@"
fi

testdir=$(mktemp -d "$root/.ci-slot-test.XXXXXXXX")
holder=
waiter1=
waiter2=
waiter3=
cleanup() {
    touch "$testdir/release"
    for pid in $holder $waiter1 $waiter2 $waiter3; do kill "$pid" 2>/dev/null || true; done
    rm -rf "$testdir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export XDG_RUNTIME_DIR="$testdir"
CARGO_BUILD_JOBS=2; export CARGO_BUILD_JOBS

fail() {
    echo "ci_slot_test.sh: $*" >&2
    exit 1
}

await_file() {
    tries=0
    until [ -e "$1" ]; do
        tries=$((tries + 1))
        [ "$tries" -le 200 ] || fail "timed out waiting for $1"
        sleep 0.1
    done
}

await_waiting() {
    tries=0
    until grep -q 'waiting for one to free' "$1"; do
        tries=$((tries + 1))
        [ "$tries" -le 200 ] || fail "timed out waiting for announcement in $1"
        sleep 0.1
    done
}

status=0
CI_HOST_SLOTS=1 sh "$slot" sh -c 'exit 3' || status=$?
[ "$status" -eq 3 ] || fail "the command's exit status $status is not passed on"
[ -e "$testdir/heavy-ci/slot-0.lock" ] || fail "no lock file for slot 0"
if CI_HOST_SLOTS=1 sh "$slot" sh -c 'echo >&9' 2>/dev/null; then
    fail "the command inherited the slot's lock descriptor"
fi
if CI_HOST_SLOTS=two sh "$slot" true 2>/dev/null; then
    fail "a malformed CI_HOST_SLOTS was accepted"
fi

CI_HOST_SLOTS=1 sh "$slot" timeout 30 sh -c "touch '$testdir/held'; until [ -e '$testdir/release' ]; do sleep 0.1; done" &
holder=$!
await_file "$testdir/held"
CI_HOST_SLOTS=2 sh "$slot" true 2>"$testdir/second.err"
[ ! -s "$testdir/second.err" ] || fail "a free second slot waited: $(cat "$testdir/second.err")"
CI_HOST_SLOTS=0 sh "$slot" true

# shellcheck disable=SC2016
env -u CI_HOST_SLOTS sh "$slot" sh -c 'echo first >> "$1"' sh "$testdir/order" 2>"$testdir/waiter1.err" &
waiter1=$!
await_waiting "$testdir/waiter1.err"
CI_HOST_SLOTS=1 sh "$slot" sh -c 'echo killed >> "$1"' sh "$testdir/order" 2>"$testdir/waiter2.err" &
waiter2=$!
await_waiting "$testdir/waiter2.err"
CI_HOST_SLOTS=1 sh "$slot" sh -c 'echo last >> "$1"' sh "$testdir/order" 2>"$testdir/waiter3.err" &
waiter3=$!
await_waiting "$testdir/waiter3.err"
[ ! -e "$testdir/order" ] || fail "a waiter ran while the only slot was held"

CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/status" || fail "--status failed"
grep -q "$holder" "$testdir/status" || fail "status omitted the holder PID"
grep -q "$waiter1" "$testdir/status" || fail "status omitted a waiter PID"
grep -qi 'holder' "$testdir/status" || fail "status omitted the holder"
grep -qi 'waiting since' "$testdir/status" || fail "status omitted waiting since"
grep -q "$testdir/order" "$testdir/status" || fail "status omitted the waiting command"

kill "$waiter2"
status=0
wait "$waiter2" || status=$?
waiter2=
[ "$status" -ne 0 ] || fail "killed waiter exited successfully"
touch "$testdir/release"
wait "$holder"
holder=
wait "$waiter1"
waiter1=
wait "$waiter3"
waiter3=
[ "$(cat "$testdir/order")" = "$(printf 'first\nlast')" ] || fail "waiters did not run in FIFO order after a waiter was killed"
[ "$(grep -c 'waiting' "$testdir/waiter1.err")" -eq 1 ] || fail "waiting was announced more than once"

# shellcheck disable=SC2016
CI_HOST_SLOTS=1 timeout 10 sh "$slot" sh -c 'sh "$1" sh -c "exit 7"' sh "$slot" && status=0 || status=$?
[ "$status" -eq 7 ] || fail "nested invocation did not run and preserve its exit status: $status"
CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/empty-status" || fail "--status failed with no holders"
echo "ci_slot_test.sh: ok"
