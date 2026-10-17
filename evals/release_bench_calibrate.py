"""Choose the frozen release set from calibration runs of the native arms.

A task is eligible when every repeat ran cleanly (no infra error) and it is
not saturated: a task both models always solve, or neither ever solves, cannot
show a harness change on either tier and only adds cost. Low-effort models are
nearly deterministic on a given task, so most tasks sit at 0% or 100% per model;
the set's mean pass rate, not each task's, is steered to the targets. Tasks
nearest the Haiku target are preferred, a subset is chosen greedily so the mean
Haiku pass rate lands near its target and Luna's near its own (lower) target,
and no template dominates.
"""
import math
from collections import Counter, defaultdict

SELECTION_DEFAULTS = {"haiku_target": 0.5, "luna_target": 0.3, "min_size": 80, "max_size": 120,
                      "max_template_share": 0.2, "min_repeats": 2}


def task_rates(records):
    """{task: {"template", "family", "haiku", "luna", "haiku_n", "luna_n", "infra", "cost"}} from calibration records."""
    grouped = defaultdict(list)
    for item in records:
        grouped[item["task"]].append(item)
    rates = {}
    for task, items in grouped.items():
        row = {"template": items[0]["template"], "family": items[0]["family"],
               "infra": sum(1 for item in items if item["infra_error"]),
               "cost": sum(item["cost_usd"] or 0 for item in items)}
        for model in ("haiku", "luna"):
            outcomes = [item["passed"] for item in items if item["model"] == model and not item["infra_error"]]
            row[model] = sum(outcomes) / len(outcomes) if outcomes else None
            row[f"{model}_n"] = len(outcomes)
        rates[task] = row
    assert all(row["haiku_n"] + row["luna_n"] + row["infra"] > 0 for row in rates.values()), "every task ran"
    return rates


def _saturated(row):
    return row["haiku"] == row["luna"] and row["haiku"] in (0, 1)


def eligible(rates, settings):
    assert settings["min_repeats"] >= 1, "a rate needs at least one counted repeat"
    chosen = sorted(task for task, row in rates.items()
                    if row["infra"] == 0 and row["haiku_n"] >= settings["min_repeats"]
                    and row["luna_n"] >= settings["min_repeats"] and not _saturated(row))
    assert all(rates[task]["haiku"] is not None and rates[task]["luna"] is not None for task in chosen)
    return chosen


def _distance(rates, chosen, settings):
    haiku = sum(rates[task]["haiku"] for task in chosen) / len(chosen)
    luna = sum(rates[task]["luna"] for task in chosen) / len(chosen)
    return abs(haiku - settings["haiku_target"]) + abs(luna - settings["luna_target"])


def select(rates, settings=None):
    """The chosen task ids: eligible, template-capped, then trimmed toward the targets while above min_size."""
    settings = {**SELECTION_DEFAULTS, **(settings or {})}
    cap = max(1, math.floor(settings["max_template_share"] * settings["max_size"]))
    per_template = defaultdict(list)
    for task in eligible(rates, settings):
        per_template[rates[task]["template"]].append(task)
    chosen = []
    for template, tasks in sorted(per_template.items()):
        # Within a template keep the tasks nearest the Haiku target first.
        tasks.sort(key=lambda task: (abs(rates[task]["haiku"] - settings["haiku_target"]), task))
        chosen += tasks[:cap]
    # Drop the task whose removal best moves the means toward the targets; above max_size always drop.
    while len(chosen) > settings["min_size"]:
        best = min(chosen, key=lambda task: (_distance(rates, [t for t in chosen if t != task], settings), task))
        improves = _distance(rates, [t for t in chosen if t != best], settings) < _distance(rates, chosen, settings)
        if len(chosen) <= settings["max_size"] and not improves:
            break
        chosen.remove(best)
    assert len(set(chosen)) == len(chosen), "a task is chosen once"
    assert all(Counter(rates[task]["template"] for task in chosen)[rates[task]["template"]] <= cap
               for task in chosen), "no template exceeds its cap"
    return sorted(chosen)


def summary(rates, chosen):
    """Mean pass rates of the chosen set and its make-up by template."""
    assert chosen, "summarize a non-empty selection"
    return {"tasks": len(chosen),
            "haiku_pass": sum(rates[task]["haiku"] for task in chosen) / len(chosen),
            "luna_pass": sum(rates[task]["luna"] for task in chosen) / len(chosen),
            "templates": dict(sorted(Counter(rates[task]["template"] for task in chosen).items()))}


def render(rates, chosen, settings, header):
    """calibration.md: per-template outcome of the calibration and the chosen set."""
    settings = {**SELECTION_DEFAULTS, **(settings or {})}
    pool = eligible(rates, settings)
    lines = ["# Release-set calibration", "", *header, "",
             f"Eligible: no infra error, at least {settings['min_repeats']} counted repeats per model, and not "
             "saturated (solved every time by both native models, or never by either).", "",
             "| Template | Candidates | Haiku pass | Luna pass | Eligible | Chosen |", "|---|---:|---:|---:|---:|---:|"]
    by_template = defaultdict(list)
    for task, row in rates.items():
        by_template[row["template"]].append(task)
    for template, tasks in sorted(by_template.items()):
        haiku = [rates[t]["haiku"] for t in tasks if rates[t]["haiku"] is not None]
        luna = [rates[t]["luna"] for t in tasks if rates[t]["luna"] is not None]
        lines.append(f"| {template} | {len(tasks)} | {_mean_pct(haiku)} | {_mean_pct(luna)} "
                     f"| {sum(t in pool for t in tasks)} | {sum(t in chosen for t in tasks)} |")
    if chosen:
        result = summary(rates, chosen)
        lines += ["", f"Chosen: **{result['tasks']} tasks**, mean native Haiku pass rate "
                  f"**{result['haiku_pass']:.0%}**, mean native Luna pass rate **{result['luna_pass']:.0%}**."]
    else:
        lines += ["", "No task was eligible."]
    return "\n".join(lines) + "\n"


def _mean_pct(values):
    return f"{sum(values) / len(values):.0%}" if values else "–"
