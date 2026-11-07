#!/bin/sh
# Run a command in a host-wide CI slot shared with other repositories.
set -eu

slots=${CI_HOST_SLOTS:-1}
case "$slots" in
    '' | *[!0-9]*) echo "ci-slot: CI_HOST_SLOTS must be a whole number, not '$slots'" >&2; exit 2 ;;
esac
dir="${XDG_RUNTIME_DIR:-/tmp}/heavy-ci"

process_start() { sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
process_alive() {
    [ -r "/proc/$1/stat" ] && [ "$(process_start "$1")" = "$2" ] &&
        [ "$(awk '{print $3}' "/proc/$1/stat")" != Z ]
}
same_file() (
    first=$(stat -L -c '%d:%i' -- "$1" 2>/dev/null) || return 1
    second=$(stat -L -c '%d:%i' -- "$2" 2>/dev/null) || return 1
    [ "$first" = "$second" ]
)

# The marker is holder-pid:start-ticks:lock-path; other slot clients may export it.
# A nested invocation must be a descendant of the live process holding that lock.
already_held() {
    marker=${AETHER_CI_SLOT_HELD:-}
    [ -n "$marker" ] || return 1
    owner=${marker%%:*}; rest=${marker#*:}; started=${rest%%:*}; lock=${rest#*:}
    case "$owner:$started" in *[!0-9:]* | :* | *:) return 1 ;; esac
    case "$lock" in "$dir"/slot-*.lock) ;; *) return 1 ;; esac
    process_alive "$owner" "$started" || return 1
    same_file "/proc/$owner/fd/9" "$lock" || return 1
    ancestor=$$
    while [ "$ancestor" -gt 1 ]; do
        [ "$ancestor" -eq "$owner" ] && return 0
        ancestor=$(awk '/^PPid:/ {print $2}' "/proc/$ancestor/status" 2>/dev/null) || return 1
        [ -n "$ancestor" ] || return 1
    done
    return 1
}

queue_lock() { exec 8>"$dir/queue.lock"; flock 8; }
queue_unlock() { flock -u 8; exec 8>&-; }
prune_queue() {
    for prune_file in "$dir"/queue/*; do
        [ -f "$prune_file" ] || continue
        pid=; start=; since=
        IFS=' ' read -r pid start since < "$prune_file" || :
        case "$pid:$start" in
            *[!0-9:]* | :* | *:) rm -f "$prune_file"; continue ;;
        esac
        if [ -z "$since" ] || ! process_alive "$pid" "$start"; then rm -f "$prune_file"; fi
    done
}

holder_description() {
    lockfile=$1
    holder=; holder_start=
    if [ -r "$lockfile.owner" ]; then
        read -r holder holder_start < "$lockfile.owner" || holder=
        case "$holder:$holder_start" in
            *[!0-9:]* | :* | *:) holder= ;;
            *) process_alive "$holder" "$holder_start" && same_file "/proc/$holder/fd/9" "$lockfile" || holder= ;;
        esac
    fi
    if [ -z "$holder" ]; then
        holder=$(lslocks -n -o PID,PATH 2>/dev/null | awk -v path="$lockfile" '$2 == path {print $1; exit}')
    fi
    case "$holder" in '' | *[!0-9]*) printf 'unknown holder'; return ;; esac
    if [ -r "/proc/$holder/stat" ]; then
        command=$(tr '\000' ' ' 2>/dev/null < "/proc/$holder/cmdline") || command=
        [ -n "$command" ] || command=$(ps -p "$holder" -o comm= 2>/dev/null) || command=
        age=$(ps -p "$holder" -o etimes= 2>/dev/null | tr -d ' ') || age=
        since=$(ps -p "$holder" -o lstart= 2>/dev/null) || since=
        if [ -z "$command" ] || [ -z "$since" ]; then printf 'unknown holder'; return; fi
        printf 'pid %s, command %s, start %s, age %ss' "$holder" "$command" "$since" "$age"
        processes=$(ps -eo pid=,ppid=,time=,pcpu=) || return
        cpu=$(printf '%s\n' "$processes" | awk -v root="$holder" '
            { p[$1]=$2; t[$1]=$3; cpu[$1]=$4 } END {
                for (id in p) { a=id; while (a>1 && a!=root && p[a]!=a) a=p[a];
                    if (a==root) { days=split(t[id],d,"-"); n=split(d[days],v,":"); s=0;
                        for(i=1;i<=n;i++) s=s*60+v[i]; if (days==2) s+=d[1]*86400; total+=s;
                        if (cpu[id]>0) busy=1 } }
                if (busy) total+=5;
                print total+0
            }')
        case "$age" in '' | *[!0-9]*) ;; *)
            if [ "$age" -ge 300 ] && [ "$cpu" -lt 5 ]; then printf ' (idle holder)'; fi ;;
        esac
    else
        printf 'unknown holder'
    fi
}

show_status() {
    printf 'Slots in %s:\n' "$dir"
    i=0
    while [ "$i" -lt "$slots" ]; do
        file="$dir/slot-$i.lock"
        exec 9>>"$file"
        if flock -n 9; then printf 'slot %s: free\n' "$i"; flock -u 9
        else printf 'slot %s holder: ' "$i"; holder_description "$file"; printf '\n'; fi
        exec 9>&-
        i=$((i + 1))
    done
    printf 'Queue:\n'
    for ticket in "$dir"/queue/*; do
        [ -f "$ticket" ] || continue
        IFS=' ' read -r pid start since < "$ticket" || continue
        if process_alive "$pid" "$start"; then
            printf 'pid %s, waiting since %s, command ' "$pid" "$since"
            tr '\000' ' ' 2>/dev/null < "/proc/$pid/cmdline" || printf 'unknown command'; printf '\n'
        fi
    done
    awk '/^MemAvailable:|^SwapTotal:|^SwapFree:/ {v[$1]=$2} END {printf "MemAvailable: %s kB; swap in use: %s kB\n", v["MemAvailable:"], v["SwapTotal:"]-v["SwapFree:"]}' /proc/meminfo
    if [ -r /proc/pressure/io ]; then printf 'IO pressure: '; tr '\n' ' ' < /proc/pressure/io; printf '\n'; fi
}

set_jobs() {
    [ "${CARGO_BUILD_JOBS+x}" ] && return
    available=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
    cores=$(getconf _NPROCESSORS_ONLN)
    jobs=$((available / 1258291))
    [ "$jobs" -ge 2 ] || jobs=2
    [ "$jobs" -le "$cores" ] || jobs=$cores
    CARGO_BUILD_JOBS=$jobs; export CARGO_BUILD_JOBS
    echo "ci-slot: CARGO_BUILD_JOBS=$jobs (memory-sized)" >&2
}

[ "$#" -gt 0 ] || { echo "usage: $0 --status | command [argument...]" >&2; exit 2; }
if [ "$1" != --status ] && { [ "$slots" -eq 0 ] || already_held; }; then exec "$@"; fi
if ! command -v flock >/dev/null 2>&1; then
    [ "$1" != --status ] || { echo 'ci-slot: status needs flock' >&2; exit 1; }
    echo 'ci-slot: no flock on PATH (util-linux); running without a host CI slot' >&2
    exec "$@"
fi
mkdir -p "$dir/queue"
if [ "$1" = --status ]; then show_status; exit 0; fi

queue_lock
prune_queue
counter=0
if [ -f "$dir/counter" ]; then
    read -r counter < "$dir/counter" || { echo "ci-slot: invalid queue counter" >&2; exit 1; }
fi
case "$counter" in '' | *[!0-9]*) echo "ci-slot: invalid queue counter" >&2; exit 1 ;; esac
counter=$((counter + 1))
printf '%s\n' "$counter" > "$dir/counter"
ticket=$(printf '%s/queue/%020d' "$dir" "$counter")
printf '%s %s %s\n' "$$" "$(process_start $$)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$ticket"
queue_unlock

told=
while :; do
    queue_lock
    prune_queue
    [ -f "$ticket" ] || { echo "ci-slot: queue ticket disappeared: $ticket" >&2; exit 1; }
    position=0
    for entry in "$dir"/queue/*; do
        [ -f "$entry" ] || continue
        position=$((position + 1))
        [ "$entry" != "$ticket" ] || break
    done
    if [ "$position" -eq 1 ]; then
        i=0
        while [ "$i" -lt "$slots" ]; do
            file="$dir/slot-$i.lock"
            exec 9>>"$file"
            if flock -n 9; then
                started=$(process_start $$)
                printf '%s %s\n' "$$" "$started" > "$file.owner.$$"
                mv -f "$file.owner.$$" "$file.owner"
                rm -f "$ticket"
                queue_unlock
                AETHER_CI_SLOT_HELD="$$:$started:$file"; export AETHER_CI_SLOT_HELD
                set_jobs
                status=0
                (exec 9>&-; exec "$@") || status=$?
                exit "$status"
            fi
            exec 9>&-
            i=$((i + 1))
        done
    fi
    if [ -z "$told" ]; then
        printf 'ci-slot: all %s host CI slots in %s are busy; waiting for one to free (queue position %s; ' "$slots" "$dir" "$position" >&2
        holder_description "$dir/slot-0.lock" >&2
        printf ')\n' >&2
        told=1
    fi
    queue_unlock
    sleep 2
done
