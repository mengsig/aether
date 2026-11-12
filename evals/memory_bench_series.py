#!/usr/bin/env python3
"""Summarise `memory_bench_series.sh` runs: the acceptance metrics per arm,
with the latency of every run and their median, spread and relative range.

  python3 evals/memory_bench_series.py <out-dir> <runs>

Writes <out-dir>/series.json and prints a table.
"""
import json
import sys
from pathlib import Path

DETERMINISTIC = (("retrieval", "precision@3"), ("retrieval", "precision@6"),
                 ("injection", "tokens_on_queries_without_relevant"), ("injection", "tokens_per_turn_mean"))
LATENCY = ("median", "p95", "scaled_median", "scaled_p95")


def median(values):
    ordered = sorted(values)
    assert ordered, "no runs"
    mid = len(ordered) // 2
    return ordered[mid] if len(ordered) % 2 else (ordered[mid - 1] + ordered[mid]) / 2


def summarise(results):
    """Per arm: deterministic metrics (identical in every run) and latency series."""
    arms = {}
    for arm in results[0]["arms"]:
        runs = [r["arms"][arm] for r in results]
        det = {}
        for group, name in DETERMINISTIC:
            values = {run["deterministic"][group][name] for run in runs}
            assert len(values) == 1, f"{arm} {name} differs between runs: {values}"
            det[name] = values.pop()
        lat = {}
        for name in LATENCY:
            series = [run["measured"]["latency_us"][name] for run in runs]
            mid = median(series)
            lat[name] = {"runs": series, "median": round(mid, 3),
                         "min": min(series), "max": max(series),
                         "range_pct": round(100 * (max(series) - min(series)) / mid, 1)}
        arms[arm] = {"deterministic": det, "latency_us": lat}
    return {"commits": sorted({r["commit"] for r in results}), "runs": len(results), "arms": arms}


def render(summary):
    lines = [f"runs: {summary['runs']}, commits: {', '.join(summary['commits'])}", ""]
    for arm, s in summary["arms"].items():
        lines.append(f"## index={arm}")
        for name, value in s["deterministic"].items():
            lines.append(f"- {name}: {value}")
        lines += ["", "| latency us | runs | median | range % |", "|---|---|---|---|"]
        for name, st in s["latency_us"].items():
            runs = ", ".join(f"{v:.0f}" for v in st["runs"])
            lines.append(f"| {name} | {runs} | {st['median']:.1f} | {st['range_pct']} |")
        lines.append("")
    return "\n".join(lines)


def main(argv):
    if len(argv) != 2:
        raise SystemExit(__doc__)
    out, runs = Path(argv[0]), int(argv[1])
    results = [json.loads((out / f"run{i}" / "results.json").read_text()) for i in range(1, runs + 1)]
    summary = summarise(results)
    (out / "series.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(render(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
