#!/bin/sh
# Exercise the host CI slots of ci_slot.sh against a private runtime dir.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
slot="$root/scripts/ci_slot.sh"
if ! command -v flock >/dev/null 2>&1; then
    echo "SKIP: ci_slot_test.sh needs flock (util-linux)"
    exit 0
fi
testdir=$(mktemp -d)
holder=
waiter=
cleanup() {
    for pid in $holder $waiter; do kill "$pid" 2>/dev/null || true; done
    rm -rf "$testdir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export XDG_RUNTIME_DIR="$testdir"

fail() {
    echo "ci_slot_test.sh: $*" >&2
    exit 1
}

# Wait up to 20 s for a file to appear; a slot script that hangs fails here.
await() {
    tries=0
    until [ -e "$1" ]; do
        tries=$((tries + 1))
        [ "$tries" -le 200 ] || fail "timed out waiting for $1"
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

CI_HOST_SLOTS=1 sh "$slot" sh -c "touch '$testdir/held'; until [ -e '$testdir/release' ]; do sleep 0.1; done" &
holder=$!
await "$testdir/held"
CI_HOST_SLOTS=2 sh "$slot" true 2>"$testdir/second.err"
[ ! -s "$testdir/second.err" ] || fail "a free second slot waited: $(cat "$testdir/second.err")"
CI_HOST_SLOTS=0 sh "$slot" true

# The default is one slot, so the held slot 0 leaves none free.
env -u CI_HOST_SLOTS sh "$slot" touch "$testdir/ran" 2>"$testdir/waiting.err" &
waiter=$!
tries=0
until grep -q 'waiting for one to free' "$testdir/waiting.err"; do
    [ ! -e "$testdir/ran" ] || fail "ran while the only slot was held"
    tries=$((tries + 1))
    [ "$tries" -le 200 ] || fail "never said it waits for a slot"
    sleep 0.1
done
[ ! -e "$testdir/ran" ] || fail "ran while the only slot was held"
touch "$testdir/release"
wait "$holder"
holder=
await "$testdir/ran"
wait "$waiter"
waiter=
[ "$(grep -c 'waiting' "$testdir/waiting.err")" -eq 1 ] || fail "waiting was announced more than once"
echo "ci_slot_test.sh: ok"
