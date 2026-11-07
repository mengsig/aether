"""The memory benchmark's metrics, computed from the dataset and one driver report.

Everything under `deterministic` depends only on the code and the dataset, so
`run --check` compares it exactly. `measured` (latency, bytes on disk) depends
on the machine and is reported, never checked.
"""
import math

from memory_bench_dataset import HARMFUL_CLASSES, cluster_roots, memory_index, queries, replacements

DIGITS = 4


def mean(values):
    values = list(values)
    return sum(values) / len(values) if values else 0.0


def percentile(values, fraction):
    """Nearest-rank percentile; 0.0 for no values."""
    ordered = sorted(values)
    if not ordered:
        return 0.0
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def first_of_cluster(keys, roots):
    """For each key, whether no earlier key restates the same memory."""
    seen = set()
    fresh = []
    for key in keys:
        root = roots.get(key, key)
        fresh.append(root not in seen)
        seen.add(root)
    return fresh


def score_query(query, row, roots, classes, ks):
    """One query's labels against the keys it ranked. Relevance is scored on
    the ranking; exposure, slots and tokens only on the lines the model got."""
    ranked = [item["key"] for item in row["ranked"]]
    assert row["id"] == query["id"], (row["id"], query["id"])
    relevant = set(query["relevant"])
    useful = [fresh and roots.get(key, key) in relevant
              for key, fresh in zip(ranked, first_of_cluster(ranked, roots))]
    shown_items = [item for item in row["ranked"] if item["injected"]]
    shown = [item["key"] for item in shown_items]
    shown_fresh = first_of_cluster(shown, roots)
    shown_useful = [fresh and roots.get(key, key) in relevant for key, fresh in zip(shown, shown_fresh)]
    assert not shown or row["layer"], f"{query['id']}: lines injected without a layer"
    scored = {"id": query["id"], "ranked": ranked, "shown": shown, "relevant": sorted(relevant)}
    for k in ks:
        top = useful[:k]
        hits = sum(top)
        scored[f"precision@{k}"] = hits / len(top) if top else (1.0 if not relevant else 0.0)
        scored[f"recall@{k}"] = hits / len(relevant) if relevant else None
    first = next((i for i, u in enumerate(useful) if u), None)
    scored["reciprocal_rank"] = (1.0 / (first + 1) if first is not None else 0.0) if relevant else None
    scored["harmful_slots"] = sum(key in query["harmful"] for key in shown)
    scored["redundant_slots"] = sum(not fresh for fresh in shown_fresh)
    scored["low_value_slots"] = sum(classes.get(key) == "one_off" for key in shown)
    scored["slots"] = len(shown)
    scored["layer_tokens"] = row["layer_tokens"]
    scored["wasted_tokens"] = sum(item["tokens"] for item, use in zip(shown_items, shown_useful) if not use)
    scored["line_tokens"] = sum(item["tokens"] for item in shown_items)
    return scored


def conflict_outcomes(dataset, scored_queries):
    """For each query labelling a replaced memory harmful and its replacement
    relevant: did the replacement reach the model, and ahead of the old one?
    Only injected lines count. No benchmark memory comes from a file, so the
    layer lists them in ranking order."""
    outcomes = []
    for query in scored_queries:
        for old, new, relation in replacements(dataset):
            if old not in query["harmful_labels"] or new not in query["relevant"]:
                continue
            shown = query["shown"]
            old_at = shown.index(old) if old in shown else None
            new_at = shown.index(new) if new in shown else None
            if old_at is None:
                outcome = "replacement_only" if new_at is not None else "neither"
            elif new_at is None or old_at < new_at:
                outcome = "harmful_first"
            else:
                outcome = "replacement_first"
            outcomes.append({"query": query["id"], "old": old, "new": new, "relation": relation, "outcome": outcome})
    return outcomes


def store_quality(dataset, report):
    """What admission kept, by class, and how much of the store's current
    memories, as the store reports them, is redundant or low value."""
    index = memory_index(dataset)
    roots = cluster_roots(dataset)
    outcomes = {row["key"]: row["outcome"] for row in report["admissions"]}
    assert set(outcomes) == set(index), "the driver admitted a different set of memories"
    current = report["current"]
    assert len(current) == report["storage"]["current"], "two different counts of current memories"
    assert all(outcomes[k] != "rejected" for k in current), "a rejected memory is current"
    by_class = {}
    for key, memory in index.items():
        entry = by_class.setdefault(memory["class"], {"offered": 0, "admitted": 0})
        entry["offered"] += 1
        entry["admitted"] += outcomes[key] != "rejected"
    clusters = {}
    for key in current:
        clusters.setdefault(roots[key], []).append(key)
    redundant = sum(len(members) - 1 for members in clusters.values())
    return {
        "by_class": dict(sorted(by_class.items())),
        "current": len(current),
        "duplicate_share": redundant / len(current) if current else 0.0,
        "low_value_share": sum(index[k]["class"] == "one_off" for k in current) / len(current) if current else 0.0,
        "harmful_current_share": sum(index[k]["class"] in HARMFUL_CLASSES for k in current) / len(current) if current else 0.0,
        "transient_rejected": _rate(by_class.get("transient"), rejected=True),
        "near_duplicates_admitted": _rate(by_class.get("near_duplicate"), rejected=False),
    }


def _rate(entry, rejected):
    if not entry or not entry["offered"]:
        return None
    admitted = entry["admitted"] / entry["offered"]
    return 1.0 - admitted if rejected else admitted


def compute(dataset, report):
    manifest = dataset["manifest"]
    ks = manifest["k"]
    roots = cluster_roots(dataset)
    classes = {key: memory["class"] for key, memory in memory_index(dataset).items()}
    labelled = queries(dataset)
    rows = {row["id"]: row for row in report["queries"]}
    assert set(rows) == {q["id"] for q in labelled}, "the driver ranked a different set of queries"
    scored = []
    for query in labelled:
        entry = score_query(query, rows[query["id"]], roots, classes, ks)
        entry["harmful_labels"] = query["harmful"]
        scored.append(entry)
    conflicts = conflict_outcomes(dataset, scored)
    deterministic = {
        "retrieval": _retrieval(scored, ks),
        "influence": _influence(scored, conflicts),
        "store": store_quality(dataset, report),
        "injection": _injection(scored),
        "recovery": _recovery(report),
    }
    return {
        "deterministic": _rounded(deterministic),
        "measured": _rounded(_measured(report)),
        "queries": scored,
        "conflicts": conflicts,
    }


def _retrieval(scored, ks):
    with_relevant = [q for q in scored if q["relevant"]]
    out = {"queries": len(scored), "queries_with_relevant": len(with_relevant)}
    for k in ks:
        out[f"precision@{k}"] = mean(q[f"precision@{k}"] for q in scored)
        out[f"recall@{k}"] = mean(q[f"recall@{k}"] for q in with_relevant)
    out["mrr"] = mean(q["reciprocal_rank"] for q in with_relevant)
    return out


def _influence(scored, conflicts):
    exposed = [q for q in scored if q["harmful_labels"]]
    slots = sum(q["slots"] for q in scored)
    lost = sum(c["outcome"] == "harmful_first" for c in conflicts)
    return {
        "queries_with_harmful_labels": len(exposed),
        "harmful_exposure_rate": mean(q["harmful_slots"] > 0 for q in exposed),
        "harmful_slot_share": sum(q["harmful_slots"] for q in scored) / slots if slots else 0.0,
        "conflicts": len(conflicts),
        "conflict_loss_rate": lost / len(conflicts) if conflicts else 0.0,
    }


def _injection(scored):
    tokens = [q["layer_tokens"] for q in scored]
    lines = sum(q["line_tokens"] for q in scored)
    slots = sum(q["slots"] for q in scored)
    negatives = [q["layer_tokens"] for q in scored if not q["relevant"]]
    return {
        "mean_slots": mean(q["slots"] for q in scored),
        "tokens_per_turn_mean": mean(tokens),
        "tokens_per_turn_p95": percentile(tokens, 0.95),
        "tokens_per_turn_max": max(tokens, default=0),
        "wasted_token_share": sum(q["wasted_tokens"] for q in scored) / lines if lines else 0.0,
        "tokens_on_queries_without_relevant": mean(negatives),
        "redundant_slot_share": sum(q["redundant_slots"] for q in scored) / slots if slots else 0.0,
        "low_value_slot_share": sum(q["low_value_slots"] for q in scored) / slots if slots else 0.0,
        "model_calls_per_retrieval": 0,
    }


def _recovery(report):
    restart = report["restart"]
    cases = {
        case["case"]: {
            "open": case["open"],
            "retrieval": case["retrieval"],
            "recovered_share": None if case["recovered"] is None or not case["expected"]
            else case["recovered"] / case["expected"],
        }
        for case in report["corruption"]
    }
    return {
        "restart_identical_share": restart["identical"] / restart["queries"] if restart["queries"] else 1.0,
        "restart_memories_kept": restart["current_after"] == restart["current_before"],
        "corruption": cases,
    }


def _measured(report):
    small = [m for row in report["latency"]["small"] for m in row["micros"]]
    scaled = report["latency"]["scaled"]
    large = [m for row in scaled["timings"] for m in row["micros"]]
    storage = report["storage"]
    empty = storage["empty"]["database"]
    return {
        "latency_us": {
            "store_memories": storage["current"],
            "median": percentile(small, 0.5),
            "p95": percentile(small, 0.95),
            "scaled_store_memories": scaled["memories"],
            "scaled_median": percentile(large, 0.5),
            "scaled_p95": percentile(large, 0.95),
        },
        "storage_bytes": {
            "empty_database": empty,
            "seeded_database": storage["seeded"]["database"],
            "seeded_on_disk": storage["seeded"]["total"],
            "per_record": _per(storage["seeded"]["database"] - empty, storage["records"]),
            "scaled_database": scaled["footprint"]["database"],
            "scaled_on_disk": scaled["footprint"]["total"],
            "scaled_per_memory": _per(scaled["footprint"]["database"] - empty, scaled["memories"]),
        },
    }


def _per(numerator, count):
    return numerator / count if count else 0.0


def _rounded(value):
    if isinstance(value, float):
        return round(value, DIGITS)
    if isinstance(value, dict):
        return {k: _rounded(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_rounded(v) for v in value]
    return value


def differences(expected, actual, prefix=""):
    """Paths where two deterministic metric trees disagree, for `--check`."""
    if isinstance(expected, dict) and isinstance(actual, dict):
        out = []
        for key in sorted(set(expected) | set(actual)):
            out += differences(expected.get(key), actual.get(key), f"{prefix}{key}.")
        return out
    return [] if expected == actual else [f"{prefix.rstrip('.')}: expected {expected!r}, got {actual!r}"]
