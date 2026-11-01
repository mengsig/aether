#!/usr/bin/env python3
"""Accuracy across releases: the analysis of a release-bench history run.

  export  a run directory's results.jsonl as the compact CSV the history page commits
  report  the history tables from one or more such CSVs, in Markdown

A history run puts many releases side by side as arms of one run (`--release`);
see docs/release-bench/history.md. Each release is compared with the newest on
the same (task, repeat) cells, and the pass rate is tested for a trend over
release order with a conditional (task fixed-effect) logistic regression and a
within-task permutation test.
"""
import argparse
import csv
import json
import math
import os
import random
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from release_bench_report import arm_summary  # noqa: E402
from release_bench_stats import _paired_tasks, bootstrap, compare, counted, mean  # noqa: E402

FIELDS = ("tier", "version", "task", "family", "repeat", "passed", "graded_pass", "checks_passed",
          "checks_total", "cost_usd", "seconds", "harness_failure", "infra_error")
REPO_ROOT = str(Path(__file__).resolve().parent.parent)
PERMUTATIONS = 20_000
ALPHA = 0.05


def version_key(tag):
    """(major, minor, patch) of a tag such as v1.0.10, so v1.0.10 sorts after v1.0.5."""
    parts = tag.lstrip("v").split(".")
    assert tag.startswith("v") and len(parts) == 3 and all(part.isdigit() for part in parts), f"bad tag {tag!r}"
    return tuple(int(part) for part in parts)


def _reason(text):
    """A failure reason fit to commit: no checkout path, at most 200 characters."""
    return (text or "").replace(REPO_ROOT, "<repo>")[:200]


def export_rows(records, old_tag, new_tag):
    """Compact rows of a run's records, the `old` and `new` arms renamed to their release tags."""
    version_key(old_tag), version_key(new_tag)
    rows = []
    for item in records:
        harness = item["harness"]
        assert harness not in ("native", "archon"), f"a history run has only release arms, not {harness}"
        version = {"old": old_tag, "new": new_tag}.get(harness, harness)
        graded = item["checks_passed"] == item["checks_total"] and not item["infra_error"]
        rows.append({"tier": item["model"], "version": version, "task": item["task"], "family": item["family"],
                     "repeat": item["repeat"], "passed": int(item["passed"]), "graded_pass": int(graded),
                     "checks_passed": item["checks_passed"], "checks_total": item["checks_total"],
                     "cost_usd": "" if item["cost_usd"] is None else f"{item['cost_usd']:.6f}",
                     "seconds": "" if item["seconds"] is None else f"{item['seconds']:.1f}",
                     "harness_failure": _reason(item.get("harness_failure")),
                     "infra_error": _reason(item["infra_error"])})
    keys = {(row["tier"], row["version"], row["task"], row["repeat"]) for row in rows}
    assert len(keys) == len(rows), "each (tier, version, task, repeat) is recorded once"
    return sorted(rows, key=lambda row: (row["tier"], version_key(row["version"]), row["task"], row["repeat"]))


def missing_cells(records, config):
    """Planned (arm, task, repeat) cells of a run's config that have no record, sorted."""
    assert config["arms"] and config["set"]["task_ids"] and config["repeats"] > 0, "a run plans arms, tasks, repeats"
    planned = {(arm, task, repeat) for arm in config["arms"] for task in config["set"]["task_ids"]
               for repeat in range(config["repeats"])}
    present = {(item["arm"], item["task"], item["repeat"]) for item in records}
    assert present <= planned, f"records outside the plan: {sorted(present - planned)[:3]}"
    return sorted(planned - present)


def check_complete(rows, tier):
    """Raise unless every version of `tier` has the same (task, repeat) cells, so none is silently short."""
    cells = defaultdict(set)
    for row in rows:
        if row["tier"] == tier:
            cells[row["version"]].add((row["task"], row["repeat"]))
    assert cells, f"no rows for tier {tier}"
    union = set().union(*cells.values())
    short = {version: len(union - mine) for version, mine in cells.items() if mine != union}
    if short:
        raise ValueError(f"{tier}: versions missing (task, repeat) cells: {short}")


def records_of(rows, graded=False):
    """Records in the shape release_bench_stats reads; `graded` counts a harness failure by its grade."""
    records = []
    for row in rows:
        assert row["passed"] in ("0", "1") and row["graded_pass"] in ("0", "1"), f"bad pass flags in {row}"
        passed = row["graded_pass" if graded else "passed"] == "1"
        total = int(row["checks_total"])
        assert total > 0 and 0 <= int(row["checks_passed"]) <= total, f"bad check counts in {row}"
        records.append({"task": row["task"], "family": row["family"], "repeat": int(row["repeat"]),
                        "arm": f"{row['tier']}/{row['version']}", "model": row["tier"], "harness": row["version"],
                        "passed": passed, "fraction": int(row["checks_passed"]) / total,
                        "cost_usd": float(row["cost_usd"]) if row["cost_usd"] else None,
                        "seconds": float(row["seconds"]) if row["seconds"] else None,
                        "harness_failure": row["harness_failure"] or None, "infra_error": row["infra_error"] or None})
    return records


def trend_points(records, index):
    """{task: [(release position, passed), ...]} of every counted trial; `index` maps a version to its position."""
    strata = defaultdict(list)
    for item in counted(records):
        assert item["harness"] in index, f"{item['harness']} is not in the release order"
        strata[item["task"]].append((index[item["harness"]], int(item["passed"])))
    assert strata, "a trend needs counted trials"
    return dict(strata)


def _sum_counts(points):
    """{subset sum of release indices: number of subsets} over the subsets of the stratum's size of successes."""
    successes = sum(passed for _, passed in points)
    table = [defaultdict(int) for _ in range(successes + 1)]
    table[0][0] = 1
    for x, _ in points:
        for size in range(successes, 0, -1):
            for total, ways in list(table[size - 1].items()):
                table[size][total + x] += ways
    assert sum(table[successes].values()) == math.comb(len(points), successes), "every subset is counted"
    return table[successes]


def _moments(beta, strata):
    """(log-likelihood, score, information) of the conditional logit at slope `beta`."""
    loglik = score = information = 0.0
    for observed, counts in strata:
        top = max(beta * total for total in counts)
        weights = {total: ways * math.exp(beta * total - top) for total, ways in counts.items()}
        norm = sum(weights.values())
        expected = sum(total * weight for total, weight in weights.items()) / norm
        variance = sum((total - expected) ** 2 * weight for total, weight in weights.items()) / norm
        loglik += beta * observed - top - math.log(norm)
        score += observed - expected
        information += variance
    return loglik, score, information


def conditional_logit(strata_points):
    """Slope of log-odds of a pass per release step, the task effects conditioned out.

    Returns (beta, standard error, signed likelihood-ratio z, informative tasks). Tasks
    every version passed, or every version failed, carry no information and drop out."""
    strata = [(sum(x for x, passed in points if passed), _sum_counts(points))
              for points in strata_points.values() if 0 < sum(passed for _, passed in points) < len(points)]
    assert strata, "no task has both a pass and a fail, so there is no trend to fit"
    beta = 0.0
    for _ in range(100):
        _, score, information = _moments(beta, strata)
        step = score / information
        beta += max(-1.0, min(1.0, step))
        if abs(step) < 1e-10:
            break
    else:
        raise RuntimeError("the conditional logit did not converge")
    loglik, score, information = _moments(beta, strata)
    assert abs(score) < 1e-6 and information > 0, "the fit ended at a maximum"
    deviance = max(0.0, 2 * (loglik - _moments(0.0, strata)[0]))
    return beta, 1 / math.sqrt(information), math.copysign(math.sqrt(deviance), beta), len(strata)


def normal_cdf(z):
    return 0.5 * math.erfc(-z / math.sqrt(2))


def _within_slope(blocks):
    """Least-squares slope of pass on release index with a fixed effect per block."""
    num = den = 0.0
    for points in blocks:
        mean_x = sum(x for x, _ in points) / len(points)
        num += sum((x - mean_x) * passed for x, passed in points)
        den += sum((x - mean_x) ** 2 for x, _ in points)
    assert den > 0, "the blocks span more than one release"
    return num / den


def permutation_trend(records, index, permutations=PERMUTATIONS, seed=0):
    """(slope in pass-rate points per release, bootstrap 95% interval, one-sided p for a decline, two-sided p).

    Under no trend the release labels of one (task, repeat) block are exchangeable, so the
    test shuffles them within each block; the interval resamples tasks."""
    cells = defaultdict(list)
    for item in counted(records):
        cells[(item["task"], item["repeat"])].append((index[item["harness"]], int(item["passed"])))
    blocks = [points for points in cells.values() if len({x for x, _ in points}) > 1]
    assert blocks, "a permutation test needs blocks with more than one release"
    observed = _within_slope(blocks)
    rng = random.Random(seed)
    low = high = 0
    for _ in range(permutations):
        shuffled = []
        for points in blocks:
            outcomes = [passed for _, passed in points]
            rng.shuffle(outcomes)
            shuffled.append([(x, passed) for (x, _), passed in zip(points, outcomes)])
        slope = _within_slope(shuffled)
        low += slope <= observed + 1e-12
        high += abs(slope) >= abs(observed) - 1e-12
    by_task = defaultdict(list)
    for (task, _), points in cells.items():
        by_task[task].append(points)
    tasks = list(by_task.values())
    interval = bootstrap(tasks, lambda sample: _within_slope([b for t in sample for b in t if len(b) > 1]), seed=seed)
    return observed, interval[1:], (low + 1) / (permutations + 1), (high + 1) / (permutations + 1)


def one_sided_decline_p(b, c):
    """Exact one-sided McNemar p that the newer arm is worse: b old-only against c new-only."""
    n = b + c
    return 1.0 if n == 0 else sum(math.comb(n, k) for k in range(b, n + 1)) / 2 ** n



def trend(records, index, seed=0):
    """Both trend tests of one tier's records, each version at its position in `index`."""
    assert len(set(index.values())) == len(index), "every version has its own position"
    beta, se, z, informative = conditional_logit(trend_points(records, index))
    slope, interval, perm_decline_p, perm_two_sided_p = permutation_trend(records, index, seed=seed)
    return {"beta": beta, "beta_low": beta - 1.959964 * se, "beta_high": beta + 1.959964 * se,
            "logit_decline_p": normal_cdf(z), "logit_two_sided_p": math.erfc(abs(z) / math.sqrt(2)),
            "informative_tasks": informative, "slope": slope, "slope_low": interval[0], "slope_high": interval[1],
            "perm_decline_p": perm_decline_p, "perm_two_sided_p": perm_two_sided_p}


def analyze_tier(rows, tier, seed=0, positions=None):
    """Per-version summaries, each version against the newest, and the trend, for one tier.

    The trend steps one per version run; `positions` ({tag: number of releases since the
    oldest}) adds the same tests with each version at its place among every release."""
    check_complete(rows, tier)
    mine = [row for row in rows if row["tier"] == tier]
    versions = sorted({row["version"] for row in mine}, key=version_key)
    assert len(versions) >= 3, "a history needs at least three releases"
    if positions and set(positions) != set(versions):
        raise ValueError(f"{tier}: --positions names {sorted(set(positions) - set(versions), key=version_key)} "
                         f"with no rows and lacks {sorted(set(versions) - set(positions), key=version_key)}")
    records, graded = records_of(mine), records_of(mine, graded=True)
    newest, oldest = versions[-1], versions[0]
    summaries = {version: arm_summary(records, f"{tier}/{version}", seed) for version in versions}
    for version in versions:
        arm = f"{tier}/{version}"
        counted_graded = [item for item in counted(graded) if item["arm"] == arm]
        summaries[version]["graded_pass"] = sum(item["passed"] for item in counted_graded) / len(counted_graded)
    against = {version: compare(records, f"{tier}/{version}", f"{tier}/{newest}", 0.95, seed)
               for version in versions[:-1]}
    for version, result in against.items():
        result["decline_p"] = one_sided_decline_p(result["old_only"], result["new_only"])
        pairs = _paired_tasks(records, f"{tier}/{version}", f"{tier}/{newest}")
        result["old_pass"], result["new_pass"] = (mean(pair[index]["pass"] for pair in pairs) for index in (0, 1))
        assert abs(result["new_pass"] - result["old_pass"] - result["pass"][0]) < 1e-9, "the paired means differ"
    steps = {version: position for position, version in enumerate(versions)}
    result = {"tier": tier, "versions": versions, "summaries": summaries, "against": against,
              "trend": trend(records, steps, seed), "graded_trend": trend(graded, steps, seed)}
    if positions:
        assert [positions[v] for v in versions] == sorted(positions[v] for v in versions), "positions keep the order"
        result["spaced_trend"] = trend(records, {version: positions[version] for version in versions}, seed)
    oldest_vs_newest = against[oldest]
    # The conditional logit is the prespecified trend test; the permutation test is reported beside it.
    result["declines"] = {"trend": result["trend"]["logit_decline_p"] < ALPHA,
                          "trend_permutation": result["trend"]["perm_decline_p"] < ALPHA,
                          "newest_below_oldest": oldest_vs_newest["decline_p"] < ALPHA}
    return result


def _pct(value):
    return f"{value:.1%}"


def _points(value):
    return f"{value * 100:+.1f}"


def version_table(result):
    lines = ["| Version | Trials | Harness failures | Infra errors | Pass rate [95% CI] | Graded pass rate "
             "| Cost / trial | Median time |", "| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: |"]
    for version in result["versions"]:
        row = result["summaries"][version]
        point, low, high = row["pass"]
        cost = "–" if row["cost_mean"] is None else f"${row['cost_mean']:.4f}"
        unpriced = f" ({row['unpriced']} unpriced)" if row["unpriced"] else ""
        lines.append(f"| {version} | {row['trials']} | {row['harness_failures']} | {row['infra']} "
                     f"| {_pct(point)} [{_pct(low)}, {_pct(high)}] | {_pct(row['graded_pass'])} "
                     f"| {cost}{unpriced} | {row['median_seconds']:.0f}s |")
    return lines


def against_table(result):
    newest = result["versions"][-1]
    lines = [f"| Version | Pairs | Version pass | {newest} pass | Version-only / {newest}-only | McNemar p "
             f"| One-sided p ({newest} worse) | {newest} - version [95% CI] |",
             "| --- | ---: | ---: | ---: | --- | ---: | ---: | --- |"]
    for version, row in result["against"].items():
        point, low, high = row["pass"]
        lines.append(f"| {version} | {row['pairs']} | {_pct(row['old_pass'])} | {_pct(row['new_pass'])} "
                     f"| {row['old_only']} / {row['new_only']} | {row['mcnemar_p']:.3f} | {row['decline_p']:.3f} "
                     f"| {_points(point)} [{_points(low)}, {_points(high)}] |")
    return lines


def trend_lines(label, row, unit="version step"):
    return [f"- **{label}.** Conditional logit (task fixed effect, {row['informative_tasks']} informative tasks): "
            f"{row['beta']:+.3f} log-odds per {unit} [95% CI {row['beta_low']:+.3f}, {row['beta_high']:+.3f}], "
            f"odds ratio x{math.exp(row['beta']):.3f}; likelihood-ratio p = {row['logit_two_sided_p']:.3f} "
            f"two-sided, {row['logit_decline_p']:.3f} one-sided for a decline.",
            f"  Within-task permutation test: {_points(row['slope'])} points per {unit} "
            f"[bootstrap 95% CI {_points(row['slope_low'])}, {_points(row['slope_high'])}]; "
            f"p = {row['perm_two_sided_p']:.3f} two-sided, {row['perm_decline_p']:.3f} one-sided for a decline."]


def render_tier(result):
    name, newest, oldest = result["tier"].capitalize(), result["versions"][-1], result["versions"][0]
    lines = [f"### {name}", "", *version_table(result), "", f"#### {name}: each version against {newest}", "",
             *against_table(result), "", f"#### {name}: trend over release order", "",
             *trend_lines("Pass rate as recorded", result["trend"]),
             *trend_lines("Graded pass rate (a harness failure counted by its grade)", result["graded_trend"])]
    if "spaced_trend" in result:
        lines += trend_lines("Pass rate as recorded, spaced by every release since the oldest",
                             result["spaced_trend"], unit="release")
    declines = result["declines"]
    def answer(flag):
        return "yes" if flag else "no"
    return lines + ["", f"- Deterioration test (one-sided, alpha {ALPHA} each): trend significantly negative by the "
                    f"conditional logit (the primary test): **{answer(declines['trend'])}** (permutation test: "
                    f"{answer(declines['trend_permutation'])}); {newest} significantly below {oldest} by McNemar: "
                    f"**{answer(declines['newest_below_oldest'])}**.", ""]


def _positions(text):
    """{tag: position} from "v1.0.0=0,v1.0.5=5"."""
    positions = {}
    for item in text.split(","):
        tag, _, number = item.partition("=")
        version_key(tag)
        if not number.isdigit():
            raise argparse.ArgumentTypeError(f"expected TAG=N, got {item!r}")
        positions[tag] = int(number)
    return positions


def read_rows(paths):
    rows = []
    for path in paths:
        with open(path, newline="") as stream:
            reader = csv.DictReader(stream)
            assert tuple(reader.fieldnames) == FIELDS, f"{path} has columns {reader.fieldnames}, not {FIELDS}"
            rows += list(reader)
    assert rows, "no rows to analyze"
    return rows


def command_export(args):
    with open(args.run_dir / "results.jsonl") as stream:
        records = [json.loads(line) for line in stream if line.strip()]
    with open(args.run_dir / "config.json") as stream:
        missing = missing_cells(records, json.load(stream))
    if missing:
        raise SystemExit(f"{len(missing)} planned trial(s) have no record, such as {missing[:3]}; finish the run first")
    rows = export_rows(records, args.old_tag, args.new_tag)
    with open(args.out, "w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=FIELDS, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    print(f"{len(rows)} rows written to {args.out}")
    return 0


def command_report(args):
    rows = read_rows(args.csv)
    for tier in sorted({row["tier"] for row in rows}):
        try:
            result = analyze_tier(rows, tier, args.seed, args.positions)
        except ValueError as error:
            raise SystemExit(f"cannot report: {error}") from error
        print("\n".join(render_tier(result)))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("export", help="a run's results.jsonl as the committed CSV")
    export.add_argument("run_dir", type=Path)
    export.add_argument("--old-tag", required=True, help="the release tag the run's old arm ran, such as v1.2.7")
    export.add_argument("--new-tag", required=True, help="the release tag the run's new arm ran, such as v1.2.8")
    export.add_argument("--out", type=Path, required=True)
    report = commands.add_parser("report", help="the history tables from exported CSVs")
    report.add_argument("csv", type=Path, nargs="+")
    report.add_argument("--seed", type=int, default=0)
    report.add_argument("--positions", type=_positions, help="each tag's number of releases since the oldest, "
                        "as v1.0.0=0,v1.0.5=5,...; adds a trend spaced by every release")
    args = parser.parse_args(argv)
    return {"export": command_export, "report": command_report}[args.command](args)


if __name__ == "__main__":
    raise SystemExit(main())
