"""Paired statistics and the release gate of the release bench.

The unit of resampling is the task: repeats of one task on one arm are
averaged first, so a task run four times does not count as four independent
tasks. Pass/fail pairs (same task, same repeat) get an exact McNemar test.
"""
import math
import random
from collections import defaultdict

BOOTSTRAP_SAMPLES = 4000
GATE_DEFAULTS = {"accuracy_margin": 0.05, "cost_margin": 0.15, "confidence": 0.95,
                 "max_infra_rate": 0.10, "min_tasks": 20}
INTERIM_LOOKS = (0.5, 0.75)


def counted(records):
    """Records that say something about the harness: infra errors are excluded, not failed."""
    return [item for item in records if not item["infra_error"]]


def per_task(records, arm):
    """{task: {"pass", "fraction", "cost", "n"}} averaged over the arm's counted repeats.

    Cost averages only the priced repeats and is None when none is priced."""
    grouped = defaultdict(list)
    for item in counted(records):
        if item["arm"] == arm:
            grouped[item["task"]].append(item)
    result = {}
    for task, items in grouped.items():
        priced = [item["cost_usd"] for item in items if item["cost_usd"] is not None]
        result[task] = {"pass": sum(item["passed"] for item in items) / len(items),
                        "fraction": sum(item["fraction"] for item in items) / len(items),
                        "cost": sum(priced) / len(priced) if priced else None, "n": len(items)}
    assert all(0 <= row["pass"] <= 1 and 0 <= row["fraction"] <= 1 for row in result.values()), "rates are in [0, 1]"
    return result


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
    outcomes = defaultdict(dict)
    for item in counted(records):
        if item["arm"] in (old_arm, new_arm):
            outcomes[(item["task"], item["repeat"])][item["arm"]] = item["passed"]
    pairs = [pair for pair in outcomes.values() if len(pair) == 2]
    b = sum(1 for pair in pairs if pair[old_arm] and not pair[new_arm])
    c = sum(1 for pair in pairs if pair[new_arm] and not pair[old_arm])
    assert b + c <= len(pairs), "discordant pairs are a subset of the pairs"
    return b, c, len(pairs)


def _paired_tasks(records, old_arm, new_arm):
    old, new = per_task(records, old_arm), per_task(records, new_arm)
    return [(old[task], new[task]) for task in sorted(set(old) & set(new))]


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


def infra_rate(records, arms):
    chosen = [item for item in records if item["arm"] in arms]
    return sum(1 for item in chosen if item["infra_error"]) / len(chosen) if chosen else 0.0


def gate(records, old_arm, new_arm, settings=None, seed=0):
    """The release verdict for one model tier: PASS, FAIL or INCONCLUSIVE (which also blocks a release).

    FAIL when, at the configured confidence, the candidate's pass rate is more than
    `accuracy_margin` below the old release's, or its cost more than `cost_margin` above it."""
    settings = {**GATE_DEFAULTS, **(settings or {})}
    assert 0 < settings["accuracy_margin"] < 1 and settings["cost_margin"] > 0, "margins are positive"
    # A one-sided bound at `confidence` is a two-sided interval at 2 * confidence - 1.
    comparison = compare(records, old_arm, new_arm, 2 * settings["confidence"] - 1, seed)
    infra = infra_rate(records, (old_arm, new_arm))
    reasons, warnings = [], []
    if comparison["tasks"] < settings["min_tasks"]:
        reasons.append(f"only {comparison['tasks']} paired tasks (< {settings['min_tasks']})")
    if infra > settings["max_infra_rate"]:
        reasons.append(f"infra error rate {infra:.0%} > {settings['max_infra_rate']:.0%}")
    if reasons:
        return {"verdict": "INCONCLUSIVE", "reasons": reasons, "warnings": warnings, "infra_rate": infra,
                "comparison": comparison, "settings": settings}
    point, _, high = comparison["pass"]
    if high < -settings["accuracy_margin"]:
        reasons.append(f"pass rate {point:+.1%} (upper bound {high:+.1%} < -{settings['accuracy_margin']:.0%})")
    elif point < -settings["accuracy_margin"]:
        warnings.append(f"pass rate point estimate {point:+.1%} is below -{settings['accuracy_margin']:.0%}")
    if comparison["cost_ratio"] is None:
        warnings.append("cost could not be compared (no task priced on both arms)")
    else:
        ratio, low, _ = comparison["cost_ratio"]
        if low > 1 + settings["cost_margin"]:
            reasons.append(f"cost x{ratio:.2f} (lower bound x{low:.2f} > x{1 + settings['cost_margin']:.2f})")
        elif ratio > 1 + settings["cost_margin"]:
            warnings.append(f"cost point estimate x{ratio:.2f} is above x{1 + settings['cost_margin']:.2f}")
    return {"verdict": "FAIL" if reasons else "PASS", "reasons": reasons, "warnings": warnings,
            "infra_rate": infra, "comparison": comparison, "settings": settings}


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
