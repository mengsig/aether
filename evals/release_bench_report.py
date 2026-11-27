"""The Markdown report of a release-bench run: arms, the Aether and Archon gates per model, parity and drift."""
import statistics
from collections import defaultdict

from release_bench_outcomes import digest_lines, failure_digest, parity, parity_line

from release_bench_stats import (
    bootstrap,
    compare,
    counted,
    gate,
    matrix_gaps,
    mean,
    per_task,
)


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
            "harness_failures": sum(1 for item in good if item.get("harness_failure")),
            "pass": pass_ci, "fraction": mean(row["fraction"] for row in tasks),
            "cost_mean": mean(costs), "cost_total": sum(costs), "unpriced": len(good) - len(costs),
            "median_seconds": statistics.median(seconds) if seconds else None}


def _arm_table(summaries, planned):
    lines = ["| Arm | Trials / planned | Infra errors | Harness failures | Pass rate [95% CI] | Check fraction "
             "| Cost / trial | Total cost | Median time |", "|---|---:|---:|---:|---|---:|---:|---:|---:|"]
    for row in summaries:
        unpriced = f" ({row['unpriced']} unpriced)" if row["unpriced"] else ""
        seconds = "–" if row["median_seconds"] is None else f"{row['median_seconds']:.0f}s"
        lines.append(f"| {row['arm']} | {row['trials']} / {planned} | {row['infra']} | {row['harness_failures']} "
                     f"| {_interval(row['pass'], _pct)} "
                     f"| {_pct(row['fraction'])} | {_usd(row['cost_mean'])} | {_usd(row['cost_total'])}{unpriced} "
                     f"| {seconds} |")
    return lines


def _comparison_lines(label, result):
    ratio = _interval(result["cost_ratio"], lambda value: f"x{value:.2f}")
    return [f"- **{label}** ({result['new']} vs {result['old']}, {result['tasks']} tasks, {result['pairs']} pairs): "
            f"pass {_interval(result['pass'], _signed)}, check fraction {_interval(result['fraction'], _signed)}, "
            f"cost {ratio}; McNemar old-only {result['old_only']} / new-only {result['new_only']}, "
            f"p = {result['mcnemar_p']:.3f}"]


def _gate_lines(model, verdict, label="candidate vs old"):
    lines = [f"### {model}: **{verdict['verdict']}**", ""]
    lines += _comparison_lines(label, verdict["comparison"])
    lines += ["- Infra error rate: " + ", ".join(f"{arm} {rate:.1%}" for arm, rate in verdict["infra_rate"].items())]
    label = "Fails because" if verdict["verdict"] == "FAIL" else "Inconclusive because"
    lines += [f"- {label}: {reason}" for reason in verdict["reasons"]]
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


def gate_verdicts(records, models, settings, expected, seed=0, old="old", new="new"):
    """{model: gate result} for every model tier the plan gave both an `old` and a `new` arm.

    A tier that was planned but never ran is judged on no records, so it is INCONCLUSIVE, not left out."""
    planned = set(expected["arms"])
    assert {item["arm"] for item in records} <= planned, "every record belongs to a planned arm"
    return {model: gate(records, f"{model}/{old}", f"{model}/{new}", settings, seed, expected)
            for model in models if {f"{model}/{old}", f"{model}/{new}"} <= planned}


def overall_verdict(verdicts, gaps=()):
    """FAIL when any tier fails; PASS only when every tier passes and every planned arm finished
    (`gaps` lists the arms that did not); else INCONCLUSIVE, also with no tier at all."""
    outcomes = {verdict["verdict"] for verdict in verdicts.values()}
    assert outcomes <= {"PASS", "FAIL", "INCONCLUSIVE"}, f"unknown tier verdicts: {outcomes}"
    if "FAIL" in outcomes:
        return "FAIL"
    return "PASS" if outcomes == {"PASS"} and not gaps else "INCONCLUSIVE"


def archon_verdict(gates, parities, gaps=()):
    """The Archon release: FAIL on a failed regression gate or a missed parity, PASS only when every
    planned gate passes and every parity holds on a finished matrix, else INCONCLUSIVE; None if unplanned."""
    if not gates and not parities:
        return None
    statuses = {result["status"] for result in parities.values()}
    assert statuses <= {"MET", "MISSED", "UNKNOWN"}, f"unknown parity statuses: {statuses}"
    if "MISSED" in statuses or any(verdict["verdict"] == "FAIL" for verdict in gates.values()):
        return "FAIL"
    complete = gates and set(gates) == set(parities) and statuses == {"MET"}
    return "PASS" if complete and overall_verdict(gates, gaps) == "PASS" else "INCONCLUSIVE"


def combined_verdict(products):
    """FAIL when a judged product fails, PASS when every judged product passes, else INCONCLUSIVE."""
    judged = {name: verdict for name, verdict in products.items() if verdict is not None}
    if "FAIL" in judged.values():
        return "FAIL"
    return "PASS" if judged and set(judged.values()) == {"PASS"} else "INCONCLUSIVE"


def _per_model_table(records, models, arms, seed):
    """Pass rate, cost per trial and median wall of every planned arm, one row per model tier."""
    harnesses = list(dict.fromkeys(arm.split("/")[1] for arm in arms))
    lines = ["| Model | " + " | ".join(harnesses) + " |", "|---|" + "---|" * len(harnesses)]
    for model in models:
        cells = []
        for harness in harnesses:
            arm = f"{model}/{harness}"
            if arm not in arms:
                cells.append("–")
                continue
            row = arm_summary(records, arm, seed)
            rate = None if row["pass"] is None else row["pass"][0]
            wall = "–" if row["median_seconds"] is None else f"{row['median_seconds']:.0f}s"
            cells.append(f"{_pct(rate)} · {_usd(row['cost_mean'])} · {wall}")
        if any(cell != "–" for cell in cells):
            lines.append(f"| {model} | " + " | ".join(cells) + " |")
    return lines + ["", "Each cell: pass rate · mean cost per trial · median wall per trial."]


def _per_task_table(records, model, arms):
    """Per task, on one model tier: passes / counted trials, mean cost and median wall of each arm."""
    mine = [arm for arm in arms if arm.startswith(f"{model}/")]
    grouped = defaultdict(list)
    for item in counted(records):
        if item["arm"] in mine:
            grouped[(item["task"], item["arm"])].append(item)
    tasks = sorted({task for task, _ in grouped})
    lines = ["| Task | " + " | ".join(arm.split("/")[1] for arm in mine) + " |", "|---|" + "---|" * len(mine)]
    for task in tasks:
        cells = [_task_cell(grouped.get((task, arm), [])) for arm in mine]
        lines.append(f"| {task} | " + " | ".join(cells) + " |")
    return lines


def _task_cell(items):
    if not items:
        return "–"
    costs = [item["cost_usd"] for item in items if item["cost_usd"] is not None]
    seconds = [item["seconds"] for item in items if item["seconds"] is not None]
    wall = f"{statistics.median(seconds):.0f}s" if seconds else "–"
    return f"{sum(item['passed'] for item in items)}/{len(items)} · {_usd(mean(costs))} · {wall}"


def _load_lines(records, arms, load_control):
    """How load was controlled, and the load each arm's trials started under, so wall times can be compared."""
    lines = [f"- {load_control}"] if load_control else []
    lines += ["", "| Arm | Median 1-min load at start | Highest |", "|---|---:|---:|"]
    for arm in arms:
        loads = sorted(item["load_at_start"] for item in records
                       if item["arm"] == arm and item.get("load_at_start") is not None)
        if loads:
            lines.append(f"| {arm} | {statistics.median(loads):.1f} | {loads[-1]:.1f} |")
        else:
            lines.append(f"| {arm} | – | – |")
    return lines


def _archon_sections(records, models, settings, expected, seed, gaps):
    """The Archon gate (archon-new vs archon-old) and parity (archon-new vs new); ([lines], verdict, gates, parities)."""
    planned = set(expected["arms"])
    gates = gate_verdicts(records, models, settings, expected, seed, "archon-old", "archon-new")
    parities = {model: parity(records, model) for model in models
                if {f"{model}/new", f"{model}/archon-new"} <= planned}
    verdict = archon_verdict(gates, parities, gaps)
    if verdict is None:
        return [], None, gates, parities
    lines = [f"## Archon release gate: {verdict}", "",
             "Archon passes when archon-new is no worse than archon-old (the same gate as Aether's) "
             "and, on every tier, archon-new passes at least as often as plain Aether's new.", "",
             "### Parity (archon-new vs new)", ""]
    lines += [parity_line(result) for result in parities.values()] or ["- no tier ran both new and archon-new"]
    lines.append("")
    for model, gate_result in gates.items():
        lines += _gate_lines(model, gate_result, "archon candidate vs archon old") + [""]
    return lines, verdict, gates, parities


def render(records, header, models, settings, expected, seed=0, load_control=None):
    """The full report as {"text", "overall", "products", "verdicts", "archon_verdicts", "parity", "gaps",
    "digest"}; `header` is a list of "- key: value" provenance lines, `expected` the planned matrix."""
    assert records, "a report needs at least one trial"
    arms = sorted(expected["arms"])
    verdicts = gate_verdicts(records, models, settings, expected, seed)
    gaps = matrix_gaps(records, expected)
    aether = overall_verdict(verdicts, gaps) if verdicts else None
    archon_lines, archon, archon_gates, parities = _archon_sections(records, models, settings, expected, seed, gaps)
    products = {"aether": aether, "archon": archon}
    overall = combined_verdict(products)
    digest = failure_digest(records, arms)
    planned = len(expected["tasks"]) * expected["repeats"]
    lines = [f"# Release bench: {overall}", "",
             *[f"- {name.capitalize()} release: **{verdict}**" for name, verdict in products.items() if verdict],
             *header, "", "## Arms", "",
             *_arm_table([arm_summary(records, arm, seed) for arm in arms], planned), "",
             "## Per model", "", *_per_model_table(records, models, arms, seed), ""]
    if gaps:
        lines += ["The planned matrix did not finish, so the run cannot pass:", "",
                  *[f"- {gap}" for gap in gaps], ""]
    lines += ["## Aether release gate" + (f": {aether}" if aether else ""), ""]
    if not verdicts:
        lines += ["No model tier ran both an old and a candidate arm, so there is no verdict.", ""]
    for model, verdict in verdicts.items():
        lines += _gate_lines(model, verdict) + [""]
    lines += archon_lines
    lines += _drift_lines(records, models, arms, seed)
    lines += ["## Failure digest", "", "Why trials did not pass, by category; it names no task, so it can go "
              "to whoever fixes the harness (also `failure-digest.md`).", "", *digest_lines(digest), "",
              "## Load and wall time", "", *_load_lines(records, arms, load_control), "",
              "## Pass rate by family", "", *_family_table(records, arms), ""]
    for model in models:
        if any(arm.startswith(f"{model}/") for arm in arms):
            lines += [f"## Per task: {model}", "", "Each cell: passes / counted trials · mean cost · median wall.",
                      "", *_per_task_table(records, model, arms), ""]
    return {"text": "\n".join(lines), "overall": overall, "products": products, "verdicts": verdicts,
            "archon_verdicts": archon_gates, "parity": parities, "gaps": gaps, "digest": digest}


def _drift_lines(records, models, arms, seed):
    drift = [compare(records, f"{model}/native", f"{model}/old", seed=seed) for model in models
             if {f"{model}/native", f"{model}/old"} <= set(arms)]
    if not drift:
        return []
    lines = ["## Drift reference (Aether old release vs the native CLI)", ""]
    for result in drift:
        lines += _comparison_lines(result["new"].split("/")[0], result)
    return lines + ["", "A large change here between two reports means the models or native CLIs moved, "
                    "not Aether.", ""]
