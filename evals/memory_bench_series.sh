#!/bin/sh
# Repeat the memory benchmark and profile one driver, for a before/after
# comparison. Run it inside one host CI slot so other builds cannot skew it:
#   sh scripts/ci_slot.sh sh evals/memory_bench_series.sh <driver> <out-dir> [runs] [--check]
# Writes <out-dir>/run<i>/ (results.json, report.md) and <out-dir>/profile/.
# PROFILE_DRIVER profiles with another build of the same code, for a driver
# that predates --profile.
set -eu

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <driver> <out-dir> [runs] [--check]" >&2
    exit 2
fi
driver=$1
out=$2
runs=${3:-5}
check=${4:-}
case "$runs" in
    '' | *[!0-9]*)
        echo "runs must be a whole number, not '$runs'" >&2
        exit 2
        ;;
esac
if [ ! -x "$driver" ]; then
    echo "no executable driver at $driver" >&2
    exit 2
fi
here=$(dirname "$0")
mkdir -p "$out"
i=1
while [ "$i" -le "$runs" ]; do
    echo "run $i of $runs at $(date +%T), load $(cut -d' ' -f1 /proc/loadavg)" >&2
    python3 "$here/memory_bench.py" run --driver "$driver" --out "$out/run$i" $check >"$out/run$i.stdout"
    i=$((i + 1))
done
echo "profile at $(date +%T), load $(cut -d' ' -f1 /proc/loadavg)" >&2
python3 "$here/memory_bench.py" profile --driver "${PROFILE_DRIVER:-$driver}" --out "$out/profile" >"$out/profile.stdout"
python3 "$here/memory_bench_series.py" "$out" "$runs"
