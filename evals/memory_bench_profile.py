"""Where memory retrieval spends its time: the driver's `--profile` output, summarised.

The driver times each stage of `MemoryService::retrieve` on its own (alias
resolution, structural pages, lexical pages, the feedback read, the semantic
pass) beside the whole call and the context layer. `rank` is what the whole
call spends beyond the stages' sum: scoring and sorting in memory.
"""
from memory_bench_metrics import mean, percentile

STAGES = ("resolve", "structural", "lexical", "feedback", "semantic")
COLUMNS = STAGES + ("rank", "retrieve", "layer")


def summarise(profile):
    """Per store: each stage's median, p95 and share of the mean retrieval, in us."""
    out = {"index": profile["index"], "reps": profile["reps"]}
    for store in ("small", "scaled"):
        rows = profile[store]["queries"]
        assert rows, f"the {store} store profiled no query"
        samples = {name: [] for name in COLUMNS}
        for row in rows:
            reps = len(row["retrieve"])
            assert reps == profile["reps"] and all(len(row[s]) == reps for s in STAGES), row["id"]
            for name in STAGES + ("retrieve", "layer"):
                samples[name] += row[name]
            for i in range(reps):
                staged = sum(row[s][i] for s in STAGES)
                samples["rank"].append(max(0.0, row["retrieve"][i] - staged))
        total = mean(samples["retrieve"])
        out[store] = {
            "memories": profile[store]["memories"],
            "queries": len(rows),
            "reads_per_retrieval": mean(row["reads"] for row in rows),
            "pool_mean": mean(row["pool"] for row in rows),
            "stages": {
                name: {
                    "median": round(percentile(values, 0.5), 3),
                    "p95": round(percentile(values, 0.95), 3),
                    "mean": round(mean(values), 3),
                    "share": round(mean(values) / total, 4) if total else 0.0,
                }
                for name, values in samples.items()
            },
        }
    return out


def render(summary):
    lines = [f"# Retrieval profile, index={summary['index']}, {summary['reps']} reps per query", ""]
    for store in ("small", "scaled"):
        s = summary[store]
        lines += [
            f"## {store}: {s['memories']} memories, {s['queries']} queries, "
            f"{s['reads_per_retrieval']:.1f} store reads and {s['pool_mean']:.1f} candidates per retrieval",
            "",
            "| Stage | median us | p95 us | mean us | share of retrieve |",
            "|---|---|---|---|---|",
        ]
        for name in COLUMNS:
            st = s["stages"][name]
            lines.append(f"| {name} | {st['median']:.1f} | {st['p95']:.1f} | {st['mean']:.1f} | {st['share']:.1%} |")
        lines.append("")
    return "\n".join(lines)
