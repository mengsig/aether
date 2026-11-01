#!/bin/sh
# Run a command while holding one of CI_HOST_SLOTS (default 1) host-wide CI
# slots, shared with every repository that keeps this contract; 0 disables it.
set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 command [argument...]" >&2
    exit 2
fi
slots=${CI_HOST_SLOTS:-1}
case "$slots" in
    '' | *[!0-9]*)
        echo "ci-slot: CI_HOST_SLOTS must be a whole number, not '$slots'" >&2
        exit 2
        ;;
esac
if [ "$slots" -eq 0 ]; then
    exec "$@"
fi
if ! command -v flock >/dev/null 2>&1; then
    echo "ci-slot: no flock on PATH (util-linux); running without a host CI slot" >&2
    exec "$@"
fi

dir="${XDG_RUNTIME_DIR:-/tmp}/heavy-ci"
mkdir -p "$dir"

# Each slot is a lock on descriptor 9, which the kernel releases when this
# shell exits, however it exits. The command does not inherit it, so a
# process it leaves behind cannot keep the slot.
run_in_slot() {
    status=0
    "$@" 9>&- || status=$?
    exit "$status"
}

told=
while :; do
    i=0
    while [ "$i" -lt "$slots" ]; do
        exec 9>>"$dir/slot-$i.lock"
        if flock -n 9; then
            run_in_slot "$@"
        fi
        exec 9>&-
        i=$((i + 1))
    done
    if [ -z "$told" ]; then
        echo "ci-slot: all $slots host CI slots in $dir are busy; waiting for one to free" >&2
        told=1
    fi
    sleep 2
done
