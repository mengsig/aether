"""The Markdown report of a release-bench run: arms, the release gate per model, and drift."""
import statistics
from collections import defaultdict

from release_bench_stats import bootstrap, compare, counted, gate, mean, per_task


def _pct(value):
    return "–" if value is None else f"{value:.0%}"


def _usd(value):
    return "–" if value is None else f"${value:.3f}"


def _interval(triple, fmt):
    if triple is None:
        return "–"
    point, low, high = triple
    return f"{fmt(point)} [{fmt(low)}, {fmt(high)}]"


def _signed(value):
    return f"{value:+.1%}"


def arm_summary(records, arm, seed=0):
    """One arm's headline numbers; the pass-rate interval resamples tasks."""
    mine = [item for item in records if item["arm"] == arm]
    tasks = list(per_task(records, arm).values())
    good = counted(mine)
    costs = [item["cost_usd"] for item in good if item["cost_usd"] is not None]
    seconds = [item["seconds"] for item in good if item["seconds"] is not None]
    pass_ci = bootstrap(tasks, lambda rows: mean(row["pass"] for row in rows), seed=seed) if tasks else None
    return {"arm": arm, "trials": len(mine), "infra": len(mine) - len(good), "tasks": len(tasks),
            "pass": pass_ci, "fraction": mean(row["fraction"] for row in tasks),
            "cost_mean": mean(costs), "cost_total": sum(costs), "unpriced": len(good) - len(costs),
            "median_seconds": statistics.median(seconds) if seconds else None}


def _arm_table(summaries):
    lines = ["| Arm | Trials | Infra errors | Pass rate [95% CI] | Check fraction | Cost / trial | Total cost "
             "| Median time |", "|---|---:|---:|---|---:|---:|---:|---:|"]
    for row in summaries:
        unpriced = f" ({row['unpriced']} unpriced)" if row["unpriced"] else ""
        seconds = "–" if row["median_seconds"] is None else f"{row['median_seconds']:.0f}s"
        lines.append(f"| {row['arm']} | {row['trials']} | {row['infra']} | {_interval(row['pass'], _pct)} "
                     f"| {_pct(row['fraction'])} | {_usd(row['cost_mean'])} | {_usd(row['cost_total'])}{unpriced} "
                     f"| {seconds} |")
    return lines


def _comparison_lines(label, result):
    ratio = _interval(result["cost_ratio"], lambda value: f"x{value:.2f}")
    return [f"- **{label}** ({result['new']} vs {result['old']}, {result['tasks']} tasks, {result['pairs']} pairs): "
            f"pass {_interval(result['pass'], _signed)}, check fraction {_interval(result['fraction'], _signed)}, "
            f"cost {ratio}; McNemar old-only {result['old_only']} / new-only {result['new_only']}, "
            f"p = {result['mcnemar_p']:.3f}"]


def _gate_lines(model, verdict):
    lines = [f"### {model}: **{verdict['verdict']}**", ""]
    lines += _comparison_lines("candidate vs old", verdict["comparison"])
    lines += [f"- Infra error rate (old + candidate): {verdict['infra_rate']:.1%}"]
    lines += [f"- Fails because: {reason}" for reason in verdict["reasons"]]
    lines += [f"- Warning: {warning}" for warning in verdict["warnings"]]
    return lines


def _family_table(records, arms):
    rates = defaultdict(dict)
    for arm in arms:
        grouped = defaultdict(list)
        for item in counted(records):
            if item["arm"] == arm:
                grouped[item["family"]].append(item["passed"])
        for family, outcomes in grouped.items():
            rates[family][arm] = sum(outcomes) / len(outcomes)
    lines = ["| Family | " + " | ".join(arms) + " |", "|---|" + "---:|" * len(arms)]
    for family in sorted(rates):
        lines.append(f"| {family} | " + " | ".join(_pct(rates[family].get(arm)) for arm in arms) + " |")
    return lines


def gate_verdicts(records, models, settings, seed=0):
    """{model: gate result} for every model tier with both an old and a new arm."""
    arms = {item["arm"] for item in records}
    return {model: gate(records, f"{model}/old", f"{model}/new", settings, seed)
            for model in models if {f"{model}/old", f"{model}/new"} <= arms}


def overall_verdict(verdicts):
    """FAIL when any tier fails, PASS when every tier passes, else INCONCLUSIVE (also with no tier at all)."""
    outcomes = {verdict["verdict"] for verdict in verdicts.values()}
    assert outcomes <= {"PASS", "FAIL", "INCONCLUSIVE"}, f"unknown tier verdicts: {outcomes}"
    if "FAIL" in outcomes:
        return "FAIL"
    return "PASS" if outcomes == {"PASS"} else "INCONCLUSIVE"


def render(records, header, models, settings, seed=0):
    """The full report; `header` is a list of "- key: value" provenance lines."""
    assert records, "a report needs at least one trial"
    arms = sorted({item["arm"] for item in records})
    verdicts = gate_verdicts(records, models, settings, seed)
    overall = overall_verdict(verdicts)
    lines = [f"# Release bench: {overall}", "", *header, "",
             "## Arms", "", *_arm_table([arm_summary(records, arm, seed) for arm in arms]), "", "## Release gate", ""]
    if not verdicts:
        lines += ["No model tier ran both an old and a candidate arm, so there is no verdict.", ""]
    for model, verdict in verdicts.items():
        lines += _gate_lines(model, verdict) + [""]
    drift = [compare(records, f"{model}/native", f"{model}/old", seed=seed) for model in models
             if {f"{model}/native", f"{model}/old"} <= set(arms)]
    if drift:
        lines += ["## Drift reference (Aether old release vs the native CLI)", ""]
        for result in drift:
            lines += _comparison_lines(result["new"].split("/")[0], result)
        lines += ["", "A large change here between two reports means the models or native CLIs moved, "
                  "not Aether.", ""]
    lines += ["## Pass rate by family", "", *_family_table(records, arms), ""]
    return "\n".join(lines), overall, verdicts
