"""Choose the frozen release set from calibration runs of the native arms, and judge it.

A task is eligible when every repeat ran cleanly (no infra error), each model
has enough counted repeats, and native Haiku passes it between 20% and 80% of
the time: a task Haiku always or never solves cannot show a harness change on
its tier. Within the eligible pool, tasks nearest the Haiku target are kept
first, and the subset is trimmed so the mean Haiku and Luna pass rates
approach their targets, then the slowest tasks are dropped while the release
matrix it implies would outlast its wall-time target. Last, no template may
hold more than 20% of the final set.
`acceptance` then says whether the set meets the targets; an unaccepted set is
frozen only on request, marked as such, and the runner refuses it without an
explicit override.
"""
import math
from collections import Counter, defaultdict

SELECTION_DEFAULTS = {"haiku_target": 0.5, "luna_target": 0.3, "min_size": 80, "max_size": 120,
                      "max_template_share": 0.2, "min_repeats": 2, "haiku_band": (0.2, 0.8),
                      "haiku_tolerance": 0.10, "luna_band": (0.25, 0.35), "release_jobs": 6,
                      "max_release_minutes": 60}
# Release-matrix trial seconds per native calibration second: a native, an old and a new arm per tier.
# In the v2 A/A the Aether arms took 1.0x native time on Haiku and 1.45x on Luna.
RELEASE_SECONDS_PER_NATIVE = {"haiku": 1 + 2 * 1.0, "luna": 1 + 2 * 1.45}


def task_rates(records):
    """{task: {"template", "family", "infra", "cost", and per model its pass rate, counted repeats ("_n")
    and mean solve seconds ("_seconds")}} from calibration records."""
    grouped = defaultdict(list)
    for item in records:
        grouped[item["task"]].append(item)
    rates = {}
    for task, items in grouped.items():
        row = {"template": items[0]["template"], "family": items[0]["family"],
               "infra": sum(1 for item in items if item["infra_error"]),
               "cost": sum(item["cost_usd"] or 0 for item in items)}
        for model in ("haiku", "luna"):
            good = [item for item in items if item["model"] == model and not item["infra_error"]]
            seconds = [item["seconds"] for item in good if item["seconds"] is not None]
            row[model] = sum(item["passed"] for item in good) / len(good) if good else None
            row[f"{model}_n"] = len(good)
            row[f"{model}_seconds"] = sum(seconds) / len(seconds) if seconds else None
        rates[task] = row
    assert all(row["haiku_n"] + row["luna_n"] + row["infra"] > 0 for row in rates.values()), "every task ran"
    return rates


def eligible(rates, settings):
    """Tasks with clean, repeated runs whose native Haiku pass rate lies inside the band."""
    low, high = settings["haiku_band"]
    assert settings["min_repeats"] >= 1 and 0 <= low < high <= 1, "a usable band and repeat floor"
    chosen = sorted(task for task, row in rates.items()
                    if row["infra"] == 0 and row["haiku_n"] >= settings["min_repeats"]
                    and row["luna_n"] >= settings["min_repeats"] and low <= row["haiku"] <= high)
    assert all(rates[task]["haiku"] is not None and rates[task]["luna"] is not None for task in chosen)
    return chosen


def _release_seconds(row):
    """Trial seconds one task adds to a release run, or None without calibration timings."""
    timings = [row.get(f"{model}_seconds") for model in RELEASE_SECONDS_PER_NATIVE]
    if None in timings:
        return None
    return sum(seconds * factor for seconds, factor in zip(timings, RELEASE_SECONDS_PER_NATIVE.values()))


def release_minutes(rates, chosen, jobs):
    """Wall minutes one release run of `chosen` needs on `jobs` workers if they never idle: a lower bound.

    None when a task has no recorded calibration timing (one carried from a set frozen without them)."""
    assert jobs >= 1, "a release run has at least one worker"
    per_task = [_release_seconds(rates[task]) for task in chosen]
    if None in per_task:
        return None
    assert all(seconds >= 0 for seconds in per_task), "trial time is non-negative"
    return sum(per_task) / jobs / 60


def template_cap(size, settings):
    """The most tasks one template may hold in a set of `size`: its share of the set, but at least one."""
    assert size >= 0 and 0 < settings["max_template_share"] <= 1, "a set size and a template share"
    return max(1, math.floor(settings["max_template_share"] * size))


def acceptance(rates, chosen, settings):
    """{"accepted", "reasons", ...}: whether the chosen set meets its size, mix and pass-rate targets."""
    settings = {**SELECTION_DEFAULTS, **(settings or {})}
    reasons = []
    if len(chosen) < settings["min_size"]:
        reasons.append(f"{len(chosen)} in-band tasks (< {settings['min_size']})")
    result = {"tasks": len(chosen), "haiku_pass": None, "luna_pass": None}
    if chosen:
        result.update(summary(rates, chosen))
        template, count = max(result["templates"].items(), key=lambda item: (item[1], item[0]))
        if count > settings["max_template_share"] * len(chosen):
            reasons.append(f"template {template} holds {count} of {len(chosen)} tasks "
                           f"(> {settings['max_template_share']:.0%})")
        haiku, luna = result["haiku_pass"], result["luna_pass"]
        if abs(haiku - settings["haiku_target"]) > settings["haiku_tolerance"]:
            reasons.append(f"mean Haiku pass {haiku:.0%} is not within {settings['haiku_tolerance']:.0%} "
                           f"of {settings['haiku_target']:.0%}")
        low, high = settings["luna_band"]
        if not low <= luna <= high:
            reasons.append(f"mean Luna pass {luna:.0%} is outside {low:.0%}-{high:.0%}")
        minutes = release_minutes(rates, chosen, settings["release_jobs"])
        result["release_minutes"] = minutes
        if minutes is None:
            reasons.append("a chosen task has no calibration timing, so the release run time is unknown")
        elif minutes > settings["max_release_minutes"]:
            reasons.append(f"a release run needs at least {minutes:.0f} min on {settings['release_jobs']} workers "
                           f"(> {settings['max_release_minutes']} min)")
    assert len(chosen) == len(set(chosen)), "a task is chosen once"
    result.update(accepted=not reasons, reasons=reasons)
    assert result["accepted"] == (not reasons) and (result["accepted"] <= bool(chosen)), "acceptance is consistent"
    return result


def _distance(rates, chosen, settings):
    haiku = sum(rates[task]["haiku"] for task in chosen) / len(chosen)
    luna = sum(rates[task]["luna"] for task in chosen) / len(chosen)
    return abs(haiku - settings["haiku_target"]) + abs(luna - settings["luna_target"])


def select(rates, settings=None):
    """The chosen task ids: eligible, trimmed toward the targets while above min_size, then template-capped.

    The cap holds against the final set's size, so trimming can drop below min_size; acceptance reports that."""
    settings = {**SELECTION_DEFAULTS, **(settings or {})}
    cap = template_cap(settings["max_size"], settings)
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
    chosen = _cap_templates(rates, _fit_release_time(rates, chosen, settings), settings)
    assert len(set(chosen)) == len(chosen), "a task is chosen once"
    final_cap = template_cap(len(chosen), settings)
    assert all(count <= final_cap for count in Counter(rates[task]["template"] for task in chosen).values()), \
        "no template exceeds its share of the final set"
    return sorted(chosen)


def _cap_templates(rates, chosen, settings):
    """Drop, from the template most over its share, the task farthest from the Haiku target until none is over.

    Each drop shrinks the set and so the cap; the loop still ends, as every pass removes a task."""
    kept = list(chosen)
    while kept:
        counts = Counter(rates[task]["template"] for task in kept)
        template, count = max(counts.items(), key=lambda item: (item[1], item[0]))
        if count <= template_cap(len(kept), settings):
            break
        worst = max((task for task in kept if rates[task]["template"] == template),
                    key=lambda task: (abs(rates[task]["haiku"] - settings["haiku_target"]), task))
        kept.remove(worst)
    assert set(kept) <= set(chosen), "capping only removes tasks"
    return kept


def _fit_release_time(rates, chosen, settings):
    """Drop the slowest tasks, down to min_size, while a release run would outlast its target."""
    assert settings["max_release_minutes"] > 0, "a release run has a positive time target"
    kept = sorted(chosen, key=lambda task: (_release_seconds(rates[task]) or 0, task))
    while len(kept) > settings["min_size"]:
        minutes = release_minutes(rates, kept, settings["release_jobs"])
        if minutes is None or minutes <= settings["max_release_minutes"]:
            break
        kept.pop()
    assert len(kept) >= min(len(chosen), settings["min_size"]), "trimming stops at min_size"
    return kept


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
             f"Eligible: no infra error, at least {settings['min_repeats']} counted repeats per model, and a "
             f"native Haiku pass rate from {settings['haiku_band'][0]:.0%} to {settings['haiku_band'][1]:.0%}.", "",
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
        minutes = release_minutes(rates, chosen, settings["release_jobs"])
        timing = "unknown" if minutes is None else f"at least {minutes:.0f} min"
        lines += ["", f"Chosen: **{result['tasks']} tasks**, mean native Haiku pass rate "
                  f"**{result['haiku_pass']:.0%}**, mean native Luna pass rate **{result['luna_pass']:.0%}**; "
                  f"a release run on {settings['release_jobs']} workers takes {timing}."]
    else:
        lines += ["", "No task was eligible."]
    verdict = acceptance(rates, chosen, settings)
    lines += ["", "## Acceptance: " + ("**accepted**" if verdict["accepted"] else "**NOT accepted**"), ""]
    lines += [f"- {reason}" for reason in verdict["reasons"]] or [
        f"- Targets met: at least {settings['min_size']} tasks, Haiku mean within "
        f"{settings['haiku_tolerance']:.0%} of {settings['haiku_target']:.0%}, Luna mean inside "
        f"{settings['luna_band'][0]:.0%}-{settings['luna_band'][1]:.0%}, a release run within "
        f"{settings['max_release_minutes']} min on {settings['release_jobs']} workers."]
    return "\n".join(lines) + "\n"


def _mean_pct(values):
    return f"{sum(values) / len(values):.0%}" if values else "–"
