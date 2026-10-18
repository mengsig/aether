"""Paired statistics and the release gate of the release bench.

The unit of resampling is the task: repeats of one task on one arm are
averaged first, so a task run four times does not count as four independent
tasks. Two arms are compared only on the (task, repeat) cells both counted, so
an outage on one arm never leaves the other averaging repeats it lacks.
Pass/fail pairs of those cells get an exact McNemar test.
"""
import math
import random
from collections import defaultdict

BOOTSTRAP_SAMPLES = 4000
GATE_DEFAULTS = {"accuracy_margin": 0.05, "cost_margin": 0.15, "confidence": 0.95,
                 "max_infra_rate": 0.10, "min_tasks": 20, "min_priced_share": 0.95, "sequential": False}
INTERIM_LOOKS = (0.5, 0.75)


def counted(records):
    """Records that say something about the harness: infra errors are excluded, not failed."""
    return [item for item in records if not item["infra_error"]]


def _task_means(items):
    """{"pass", "fraction", "cost", "n"} of one task's repeats on one arm.

    Cost is None unless every repeat is priced: a mean over the cheap repeats
    alone would understate the task."""
    assert items and len({item["arm"] for item in items}) == 1, "one task's repeats on one arm"
    costs = [item["cost_usd"] for item in items]
    row = {"pass": sum(item["passed"] for item in items) / len(items),
           "fraction": sum(item["fraction"] for item in items) / len(items),
           "cost": None if None in costs else sum(costs) / len(costs), "n": len(items)}
    assert 0 <= row["pass"] <= 1 and 0 <= row["fraction"] <= 1, "rates are in [0, 1]"
    return row


def per_task(records, arm):
    """{task: {"pass", "fraction", "cost", "n"}} averaged over the arm's counted repeats."""
    grouped = defaultdict(list)
    for item in counted(records):
        if item["arm"] == arm:
            grouped[item["task"]].append(item)
    return {task: _task_means(items) for task, items in grouped.items()}


def paired_cells(records, old_arm, new_arm):
    """{task: [(old record, new record), ...]}: the (task, repeat) cells both arms counted, in repeat order."""
    assert old_arm != new_arm, "a comparison needs two arms"
    cells = defaultdict(dict)
    for item in counted(records):
        if item["arm"] in (old_arm, new_arm):
            cell = cells[(item["task"], item["repeat"])]
            assert item["arm"] not in cell, f"{item['arm']} recorded {item['task']} r{item['repeat']} twice"
            cell[item["arm"]] = item
    by_task = defaultdict(list)
    for (task, _), cell in sorted(cells.items()):
        if len(cell) == 2:
            by_task[task].append((cell[old_arm], cell[new_arm]))
    return dict(by_task)


def mean(values):
    values = list(values)
    return sum(values) / len(values) if values else None


def percentile(sorted_values, q):
    """Linear-interpolated percentile of an already sorted list."""
    assert sorted_values and 0 <= q <= 1, "a percentile needs data and q in [0, 1]"
    position = q * (len(sorted_values) - 1)
    low, high = math.floor(position), math.ceil(position)
    return sorted_values[low] + (sorted_values[high] - sorted_values[low]) * (position - low)


def bootstrap(pairs, statistic, confidence=0.95, samples=BOOTSTRAP_SAMPLES, seed=0):
    """(point, low, high) of `statistic(pairs)` with a percentile interval from resampling the pairs.

    Two-sided at `confidence`, so each bound is a one-sided bound at (1 + confidence) / 2."""
    assert pairs, "bootstrap needs at least one pair"
    assert 0 < confidence < 1 and samples >= 100, "a usable interval needs a confidence and enough samples"
    rng = random.Random(seed)
    point = statistic(pairs)
    draws = sorted(statistic([pairs[rng.randrange(len(pairs))] for _ in pairs]) for _ in range(samples))
    tail = (1 - confidence) / 2
    return point, percentile(draws, tail), percentile(draws, 1 - tail)


def mcnemar_exact(b, c):
    """Two-sided exact McNemar p-value from discordant counts b (old only) and c (new only)."""
    assert b >= 0 and c >= 0, "discordant counts are non-negative"
    n = b + c
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, k) for k in range(min(b, c) + 1)) / 2 ** n
    return min(1.0, 2 * tail)


def discordant(records, old_arm, new_arm):
    """(b, c, n): pairs passed only by old, only by new, and all counted pairs, matched on task and repeat."""
    pairs = [pair for cells in paired_cells(records, old_arm, new_arm).values() for pair in cells]
    b = sum(1 for old, new in pairs if old["passed"] and not new["passed"])
    c = sum(1 for old, new in pairs if new["passed"] and not old["passed"])
    assert b + c <= len(pairs), "discordant pairs are a subset of the pairs"
    return b, c, len(pairs)


def _paired_tasks(records, old_arm, new_arm):
    """(old, new) task means over the same matched repeats, one pair per task."""
    return [(_task_means([old for old, _ in cells]), _task_means([new for _, new in cells]))
            for _, cells in sorted(paired_cells(records, old_arm, new_arm).items())]


def _difference(key):
    return lambda pairs: mean(new[key] - old[key] for old, new in pairs)


def _cost_ratio(pairs):
    old = sum(pair[0]["cost"] for pair in pairs)
    new = sum(pair[1]["cost"] for pair in pairs)
    return new / old if old > 0 else math.inf


def compare(records, old_arm, new_arm, confidence=0.95, seed=0):
    """Paired differences new - old over tasks both arms have counted results for."""
    pairs = _paired_tasks(records, old_arm, new_arm)
    b, c, n = discordant(records, old_arm, new_arm)
    result = {"old": old_arm, "new": new_arm, "tasks": len(pairs), "pairs": n, "old_only": b, "new_only": c,
              "mcnemar_p": mcnemar_exact(b, c), "pass": None, "fraction": None, "cost_ratio": None}
    if not pairs:
        return result
    result["pass"] = bootstrap(pairs, _difference("pass"), confidence, seed=seed)
    result["fraction"] = bootstrap(pairs, _difference("fraction"), confidence, seed=seed + 1)
    priced = [pair for pair in pairs if pair[0]["cost"] is not None and pair[1]["cost"] is not None]
    result["priced_tasks"] = len(priced)
    if priced and sum(pair[0]["cost"] for pair in priced) > 0:
        result["cost_ratio"] = bootstrap(priced, _cost_ratio, confidence, seed=seed + 2)
    return result


def infra_rate(records, arm):
    """The share of one arm's trials that said nothing about the harness (bench steps, provider outages)."""
    chosen = [item for item in records if item["arm"] == arm]
    return sum(1 for item in chosen if item["infra_error"]) / len(chosen) if chosen else 0.0


def missing_trials(records, arm, expected):
    """How many of the expected (task, repeat) cells of `arm` have no record at all."""
    assert expected["repeats"] > 0 and expected["tasks"], "an expected matrix has tasks and repeats"
    present = {(item["task"], item["repeat"]) for item in records if item["arm"] == arm}
    cells = {(task, repeat) for task in expected["tasks"] for repeat in range(expected["repeats"])}
    assert present <= cells, f"{arm} has trials outside the expected matrix"
    return len(cells - present)


def _fail_reasons(comparison, settings):
    """Reasons the candidate is significantly worse, and warnings for point estimates past a margin."""
    reasons, warnings = [], []
    if comparison["pass"] is not None:
        point, _, high = comparison["pass"]
        if high < -settings["accuracy_margin"]:
            reasons.append(f"pass rate {point:+.1%} (upper bound {high:+.1%} < -{settings['accuracy_margin']:.0%})")
        elif point < -settings["accuracy_margin"]:
            warnings.append(f"pass rate point estimate {point:+.1%} is below -{settings['accuracy_margin']:.0%}")
    if comparison["cost_ratio"] is not None:
        ratio, low, _ = comparison["cost_ratio"]
        if low > 1 + settings["cost_margin"]:
            reasons.append(f"cost x{ratio:.2f} (lower bound x{low:.2f} > x{1 + settings['cost_margin']:.2f})")
        elif ratio > 1 + settings["cost_margin"]:
            warnings.append(f"cost point estimate x{ratio:.2f} is above x{1 + settings['cost_margin']:.2f}")
    return reasons, warnings


def _unreliable(records, comparison, arms, settings):
    """Reasons the data cannot support any verdict: too few paired tasks, or too many outages on an arm."""
    reasons = []
    if comparison["tasks"] < settings["min_tasks"]:
        reasons.append(f"only {comparison['tasks']} paired tasks (< {settings['min_tasks']})")
    for arm in arms:
        rate = infra_rate(records, arm)
        if rate > settings["max_infra_rate"]:
            reasons.append(f"{arm} infra error rate {rate:.0%} > {settings['max_infra_rate']:.0%}")
    return reasons


def _incomplete(records, arms, expected):
    """Reasons the planned matrix did not finish: a missing tier, a budget stop or a partial repeat."""
    if expected is None:
        return []
    planned = len(expected["tasks"]) * expected["repeats"]
    missing = {arm: missing_trials(records, arm, expected) for arm in arms}
    return [f"{arm} ran {planned - count} of {planned} planned trials" for arm, count in missing.items() if count]


def matrix_gaps(records, expected):
    """Reasons any planned arm, the native and Archon references included, did not finish its trials."""
    assert expected["arms"] and len(set(expected["arms"])) == len(expected["arms"]), "a plan has distinct arms"
    assert {item["arm"] for item in records} <= set(expected["arms"]), "every record belongs to a planned arm"
    return _incomplete(records, sorted(expected["arms"]), expected)


def _unpriced(comparison, settings):
    """Why the cost gate cannot be judged: too few tasks priced on both arms, or none."""
    if not comparison["tasks"]:
        return []
    share = comparison.get("priced_tasks", 0) / comparison["tasks"]
    if comparison["cost_ratio"] is None:
        return [f"cost could not be compared ({comparison.get('priced_tasks', 0)} tasks priced on both arms)"]
    if share < settings["min_priced_share"]:
        return [f"only {share:.0%} of paired tasks are priced on both arms (< {settings['min_priced_share']:.0%})"]
    return []


def gate(records, old_arm, new_arm, settings=None, seed=0, expected=None):
    """The release verdict for one model tier: PASS, FAIL or INCONCLUSIVE (which also blocks a release).

    FAIL when, at the configured confidence, the candidate's pass rate is more than
    `accuracy_margin` below the old release's, or its cost more than `cost_margin` above it.
    `expected` ({"tasks", "repeats"}) is the planned matrix; trials missing from it make
    the verdict INCONCLUSIVE, unless a sequential look (settings["sequential"]) found a FAIL.
    Missing prices block only the PASS: a FAIL on the evidence there is stands."""
    settings = {**GATE_DEFAULTS, **(settings or {})}
    assert 0 < settings["accuracy_margin"] < 1 and settings["cost_margin"] > 0, "margins are positive"
    assert 0 < settings["min_priced_share"] <= 1, "the priced share is a fraction"
    # A one-sided bound at `confidence` is a two-sided interval at 2 * confidence - 1.
    comparison = compare(records, old_arm, new_arm, 2 * settings["confidence"] - 1, seed)
    arms = (old_arm, new_arm)
    unreliable, incomplete = _unreliable(records, comparison, arms, settings), _incomplete(records, arms, expected)
    fails, warnings = _fail_reasons(comparison, settings)
    if unreliable:
        verdict, reasons = "INCONCLUSIVE", unreliable + incomplete
        warnings += [f"would fail: {reason}" for reason in fails]
    elif fails and (not incomplete or settings["sequential"]):
        verdict, reasons = "FAIL", fails
        warnings += [f"stopped before the plan finished: {reason}" for reason in incomplete]
    elif incomplete:
        verdict, reasons = "INCONCLUSIVE", incomplete
        warnings += [f"would fail: {reason}" for reason in fails]
    else:
        reasons = _unpriced(comparison, settings)
        verdict = "INCONCLUSIVE" if reasons else "PASS"
    assert verdict != "PASS" or not (fails or unreliable or incomplete or reasons), "a PASS has nothing against it"
    return {"verdict": verdict, "reasons": reasons, "warnings": warnings,
            "infra_rate": {arm: infra_rate(records, arm) for arm in arms}, "comparison": comparison,
            "settings": settings}


def reached_looks(records, old_arm, new_arm, planned_tasks, looks=INTERIM_LOOKS):
    """How many interim looks the number of paired tasks has reached so far."""
    assert planned_tasks > 0 and all(0 < fraction < 1 for fraction in looks), "looks are fractions of a plan"
    done = len(_paired_tasks(records, old_arm, new_arm))
    return sum(1 for fraction in looks if done >= math.ceil(fraction * planned_tasks))


def sequential_confidence(confidence, looks=INTERIM_LOOKS):
    """The per-look confidence when the interim looks and the final gate split the error rate (Bonferroni)."""
    assert 0 < confidence < 1, "confidence is a probability"
    return 1 - (1 - confidence) / (len(looks) + 1)


def interim_fail(records, old_arm, new_arm, settings):
    """A decisive FAIL at an interim look, or None: early stopping never stops for a pass.

    `settings` must already hold the per-look confidence from `sequential_confidence`."""
    verdict = gate(records, old_arm, new_arm, settings)
    return verdict if verdict["verdict"] == "FAIL" else None
