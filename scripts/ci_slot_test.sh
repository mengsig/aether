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
CI_LOW_PRIORITY=0 CI_MIN_FREE_GIB=0; export CI_LOW_PRIORITY CI_MIN_FREE_GIB

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
    until grep -q 'waiting for one to free' "$1" 2>/dev/null; do
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

tries=0
while :; do
    CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/status" || fail "--status failed"
    grep -q "slot 0 holder: pid $holder, command" "$testdir/status" && break
    tries=$((tries + 1))
    [ "$tries" -le 50 ] || fail "status omitted the holder PID"
    sleep 0.1
done
grep -q "$waiter1" "$testdir/status" || fail "status omitted a waiter PID"
grep -qi 'holder' "$testdir/status" || fail "status omitted the holder"
grep -qi 'waiting since' "$testdir/status" || fail "status omitted waiting since"
grep -q "$testdir/order" "$testdir/status" || fail "status omitted the waiting command"

mkdir "$testdir/mock-bin"
cat > "$testdir/mock-bin/lslocks" <<'MOCK_LSLOCKS'
#!/bin/sh
printf '999999999 %s\n' "$XDG_RUNTIME_DIR/heavy-ci/slot-0.lock"
MOCK_LSLOCKS
chmod +x "$testdir/mock-bin/lslocks"
cat > "$testdir/mock-bin/ps" <<'MOCK_PS'
#!/bin/sh
if [ "$1" = -eo ]; then
    printf '%s 1 00:00:00 0.0\n%s %s 00:00:01 80.0\n' "$MOCK_HOLDER" "$((MOCK_HOLDER + 10000000))" "$MOCK_HOLDER"
elif [ "$4" = etimes= ]; then
    echo 301
elif [ "$4" = lstart= ]; then
    echo 'test start'
else
    exit 1
fi
MOCK_PS
chmod +x "$testdir/mock-bin/ps"
MOCK_HOLDER=$holder PATH="$testdir/mock-bin:$PATH" CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/busy-status" || fail "status failed for busy descendant"
grep -q "pid $holder, command" "$testdir/busy-status" || fail "mock status omitted holder"
if grep -q 'idle holder' "$testdir/busy-status"; then fail "busy descendant reported idle"; fi
cat > "$testdir/mock-bin/tr" <<'MOCK_TR'
#!/bin/sh
if [ "$1" = '\000' ]; then exit 1; fi
exec /usr/bin/tr "$@"
MOCK_TR
chmod +x "$testdir/mock-bin/tr"
MOCK_HOLDER=$holder PATH="$testdir/mock-bin:$PATH" CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/race-status" || fail "status failed when process details disappeared"
grep -q 'unknown holder' "$testdir/race-status" || fail "missing holder details were not reported"

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
cat > "$testdir/check-marker.sh" <<'CHECK_MARKER'
#!/bin/sh
set -eu
slot=$1
marker=$AETHER_CI_SLOT_HELD
owner=${marker%%:*}; rest=${marker#*:}; ticks=${rest%%:*}; lock=${rest#*:}
reject() {
    result=0
    AETHER_CI_SLOT_HELD=$1 timeout 1 sh "$slot" true >/dev/null 2>&1 || result=$?
    [ "$result" -eq 124 ] || { echo "invalid marker bypassed the slot: $result" >&2; exit 1; }
}
reject "999999999:1:$lock"
reject "$owner:0:$lock"
reject "$owner:$ticks:$lock.foreign"
self_ticks=$(sed 's/.*) //' "/proc/$$/stat" | awk '{print $20}')
reject "$$:$self_ticks:$lock"
printf '%s\n' "$marker" > "$2"
until [ -e "$3" ]; do sleep 0.1; done
CHECK_MARKER
CI_HOST_SLOTS=1 sh "$slot" sh "$testdir/check-marker.sh" "$slot" "$testdir/marker" "$testdir/marker-release" &
holder=$!
await_file "$testdir/marker"
result=0
AETHER_CI_SLOT_HELD=$(cat "$testdir/marker") timeout 1 sh "$slot" true >/dev/null 2>&1 || result=$?
[ "$result" -eq 124 ] || fail "non-descendant reused another holder's marker"
touch "$testdir/marker-release"
wait "$holder"
holder=
# shellcheck disable=SC2016
XDG_RUNTIME_DIR="$testdir/" CI_HOST_SLOTS=1 timeout 5 sh "$slot" sh -c 'sh "$1" true' sh "$slot" || fail "nested call with trailing slash deadlocked"

CI_HOST_SLOTS=1 sh "$slot" timeout 15 sh -c 'touch "$1"; while [ ! -e "$2" ]; do sleep 0.1; done' sh "$testdir/held-again" "$testdir/release-again" &
holder=$!
await_file "$testdir/held-again"
CI_HOST_SLOTS=1 sh "$slot" true 2>"$testdir/head.err" &
waiter1=$!
await_waiting "$testdir/head.err"
CI_HOST_SLOTS=1 sh "$slot" touch "$testdir/after-head" 2>"$testdir/after-head.err" &
waiter2=$!
await_waiting "$testdir/after-head.err"
kill -KILL "$waiter1"
wait "$waiter1" 2>/dev/null || :
waiter1=
touch "$testdir/release-again"
wait "$holder"; holder=
await_file "$testdir/after-head"
wait "$waiter2"; waiter2=
run_slot() { CI_HOST_SLOTS=1 sh "$slot" "$@"; }

# Low priority: mock wrappers log their argv and exec the rest.
lpbin="$testdir/lp-bin"
mkdir "$lpbin"
for tool in nice ionice; do
    # shellcheck disable=SC2016
    printf '#!/bin/sh\necho "%s $*" >> "$LP_LOG"\nwhile [ "$#" -gt 0 ]; do case "$1" in -n|-c) shift 2 ;; *) break ;; esac; done\nexec "$@"\n' "$tool" > "$lpbin/$tool"
done
cat > "$lpbin/systemd-run" <<'MOCK_SD'
#!/bin/sh
[ "${MOCK_SD_HANG:-0}" = 1 ] && exec sleep 30
[ "${MOCK_SD_FAIL:-0}" = 1 ] && exit 1
echo "systemd-run $*" >> "$LP_LOG"
while [ "$#" -gt 0 ]; do case "$1" in --) shift; break ;; *) shift ;; esac; done
[ "$#" -gt 0 ] || exit 0
exec "$@"
MOCK_SD
chmod +x "$lpbin"/*
LP_LOG="$testdir/lp.log"; export LP_LOG
lp() { PATH="$lpbin:$PATH" CI_LOW_PRIORITY=1 run_slot "$@"; }

: > "$LP_LOG"
lp true 2>"$testdir/lp.err"
grep -q 'systemd-run --user --scope --quiet -p CPUWeight=20 -p IOWeight=20 -p MemoryHigh=6G -- nice -n 19 ionice -c 3 true' "$LP_LOG" || fail "default wrapping wrong: $(cat "$LP_LOG")"
: > "$LP_LOG"
CI_MEMORY_HIGH=2G lp true 2>/dev/null
grep -q 'MemoryHigh=2G' "$LP_LOG" || fail "CI_MEMORY_HIGH override ignored"
if grep -q MemoryMax "$LP_LOG"; then fail "MemoryMax must never be used"; fi
status=0
lp sh -c 'exit 5' 2>/dev/null || status=$?
[ "$status" -eq 5 ] || fail "exit status lost through wrappers: $status"
if lp sh -c 'echo >&9' 2>/dev/null; then fail "wrappers leaked the lock descriptor"; fi
: > "$LP_LOG"
MOCK_SD_FAIL=1 lp true 2>"$testdir/lp.err"
grep -q 'systemd-run --user unavailable; using nice/ionice only' "$testdir/lp.err" || fail "no fallback message"
grep -q '^nice -n 19 ionice' "$LP_LOG" || grep -q '^nice -n 19' "$LP_LOG" || fail "fallback skipped nice"
grep -q '^ionice -c 3' "$LP_LOG" || fail "fallback skipped ionice"
if grep -q '^systemd-run' "$LP_LOG"; then fail "systemd-run ran after a failed probe"; fi
: > "$LP_LOG"
# The positional arguments are expanded by the inner shell.
# shellcheck disable=SC2016
MOCK_SD_HANG=1 timeout 8 sh -c 'PATH="$1:$PATH" CI_LOW_PRIORITY=1 CI_HOST_SLOTS=1 sh "$2" touch "$3"' sh "$lpbin" "$slot" "$testdir/after-hang" 2>"$testdir/lp.err" || fail "hung probe blocked CI"
[ -e "$testdir/after-hang" ] || fail "command did not run after hung probe"
grep -q 'systemd-run --user unavailable; using nice/ionice only' "$testdir/lp.err" || fail "no fallback message after hung probe"
grep -q '^nice -n 19' "$LP_LOG" || fail "hung probe fallback skipped nice"
grep -q '^ionice -c 3' "$LP_LOG" || fail "hung probe fallback skipped ionice"
: > "$LP_LOG"
CI_LOW_PRIORITY=0 PATH="$lpbin:$PATH" run_slot true 2>"$testdir/lp.err"
[ ! -s "$LP_LOG" ] || fail "CI_LOW_PRIORITY=0 still wrapped"
[ ! -s "$testdir/lp.err" ] || fail "CI_LOW_PRIORITY=0 printed: $(cat "$testdir/lp.err")"

# Slot-free paths retain priority and thread policy without waiting for memory.
noflockbin="$testdir/noflock-bin"
mkdir "$noflockbin"
ln -s "$(command -v sh)" "$noflockbin/sh"
printf '#!/bin/sh\nexit 0\n' > "$noflockbin/true"
chmod +x "$noflockbin/true"
for tool in nice ionice systemd-run; do ln -s "$lpbin/$tool" "$noflockbin/$tool"; done
ln -s "$(command -v timeout)" "$noflockbin/timeout"
for path in zero noflock; do
    slot_path="$lpbin:$PATH"; slot_count=0
    if [ "$path" = noflock ]; then slot_path=$noflockbin; slot_count=1; fi
    : > "$LP_LOG"
    # shellcheck disable=SC2016
    threads=$(env -u NEXTEST_TEST_THREADS -u RUST_TEST_THREADS PATH="$slot_path" \
        CI_HOST_SLOTS="$slot_count" CI_LOW_PRIORITY=1 CI_TEST_THREADS=2 \
        CI_MIN_FREE_GIB=99 CI_SLOT_MEMINFO="$testdir/missing" \
        sh "$slot" sh -c 'printf "%s %s" "$NEXTEST_TEST_THREADS" "$RUST_TEST_THREADS"' 2>"$testdir/free.err")
    [ "$threads" = '2 2' ] || fail "$path path test threads: $threads"
    grep -q 'CPUWeight=20 -p IOWeight=20 -p MemoryHigh=6G -- nice -n 19 ionice -c 3' "$LP_LOG" || fail "$path path skipped priority wrappers"
    : > "$LP_LOG"
    env PATH="$slot_path" CI_HOST_SLOTS="$slot_count" CI_LOW_PRIORITY=0 sh "$slot" true 2>"$testdir/free.err"
    [ ! -s "$LP_LOG" ] || fail "$path path ignored CI_LOW_PRIORITY=0"
done
rm "$noflockbin/timeout"
: > "$LP_LOG"
PATH="$noflockbin" CI_HOST_SLOTS=0 CI_LOW_PRIORITY=1 sh "$slot" true 2>"$testdir/free.err"
grep -q 'systemd-run --user unavailable; using nice/ionice only' "$testdir/free.err" || fail "missing timeout did not fall back"
grep -q '^nice -n 19' "$LP_LOG" || fail "missing timeout skipped nice"
grep -q '^ionice -c 3' "$LP_LOG" || fail "missing timeout skipped ionice"
if grep -q '^systemd-run' "$LP_LOG"; then fail "systemd-run ran without timeout"; fi

# Test threads.
# shellcheck disable=SC2016
show_threads='echo $NEXTEST_TEST_THREADS $RUST_TEST_THREADS'
threads=$(env -u NEXTEST_TEST_THREADS -u RUST_TEST_THREADS sh "$slot" sh -c "$show_threads" 2>/dev/null)
[ "$threads" = "4 4" ] || fail "default test threads '$threads'"
threads=$(env -u NEXTEST_TEST_THREADS -u RUST_TEST_THREADS CI_TEST_THREADS=2 sh "$slot" sh -c "$show_threads" 2>/dev/null)
[ "$threads" = "2 2" ] || fail "CI_TEST_THREADS override gave '$threads'"
threads=$(env -u RUST_TEST_THREADS NEXTEST_TEST_THREADS=9 sh "$slot" sh -c "$show_threads" 2>/dev/null)
[ "$threads" = "9 4" ] || fail "caller NEXTEST_TEST_THREADS not kept: '$threads'"
threads=$(env -u NEXTEST_TEST_THREADS RUST_TEST_THREADS=7 sh "$slot" sh -c "$show_threads" 2>/dev/null)
[ "$threads" = "4 7" ] || fail "caller RUST_TEST_THREADS not kept: '$threads'"

# Memory wait against a fake meminfo.
printf 'MemAvailable: %s kB\n' 8388608 > "$testdir/meminfo"
export CI_SLOT_MEMINFO="$testdir/meminfo" CI_SLOT_MEM_POLL_SECS=1
CI_MIN_FREE_GIB=6 run_slot true 2>"$testdir/mem.err"
[ ! -s "$testdir/mem.err" ] || fail "enough memory still waited: $(cat "$testdir/mem.err")"
printf 'MemAvailable: %s kB\n' 3250586 > "$testdir/meminfo"
CI_MIN_FREE_GIB=0 run_slot true 2>"$testdir/mem.err"
[ ! -s "$testdir/mem.err" ] || fail "CI_MIN_FREE_GIB=0 waited"
(sleep 2; printf 'MemAvailable: 8388608 kB\n' > "$testdir/meminfo") &
bgmem=$!
CI_MIN_FREE_GIB=6 CI_SLOT_MEM_PROGRESS_SECS=1 run_slot true 2>"$testdir/mem.err"
wait "$bgmem"
grep -q 'ci-slot: waiting for 6 GiB of free memory (MemAvailable 3.1 GiB)' "$testdir/mem.err" || fail "no wait line: $(cat "$testdir/mem.err")"
[ "$(grep -c '^ci-slot: waiting for' "$testdir/mem.err")" -eq 1 ] || fail "wait line repeated"
grep -q 'still waiting' "$testdir/mem.err" || fail "no progress line"
if grep -q WARNING "$testdir/mem.err"; then fail "warned although memory recovered"; fi
printf 'MemAvailable: %s kB\n' 3250586 > "$testdir/meminfo"
CI_MIN_FREE_GIB=6 CI_MEM_WAIT_MAX_MIN=0 run_slot touch "$testdir/ran-anyway" 2>"$testdir/mem.err"
grep -q 'WARNING.*running anyway' "$testdir/mem.err" || fail "no cap warning"
[ -e "$testdir/ran-anyway" ] || fail "command did not run after the cap"
CI_MIN_FREE_GIB=6 CI_SLOT_MEM_WAIT_MAX_SECS=2 run_slot true 2>"$testdir/mem.err"
grep -q WARNING "$testdir/mem.err" || fail "no warning after timed cap"
unset CI_SLOT_MEMINFO CI_SLOT_MEM_POLL_SECS
for bad in CI_MIN_FREE_GIB CI_MEM_WAIT_MAX_MIN CI_TEST_THREADS; do
    if env "$bad=x1" sh "$slot" true 2>/dev/null; then fail "malformed $bad accepted"; fi
done
status=0
CI_MIN_FREE_GIB=-1 sh "$slot" true 2>/dev/null || status=$?
[ "$status" -eq 2 ] || fail "malformed CI_MIN_FREE_GIB exit status $status, not 2"

CI_HOST_SLOTS=1 timeout 5 sh "$slot" --status >"$testdir/empty-status" || fail "--status failed with no holders"
echo "ci_slot_test.sh: ok"
