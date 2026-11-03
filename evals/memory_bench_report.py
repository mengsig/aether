"""Markdown for the memory benchmark: one run's report and a two-run comparison."""

# (section, metric, label, better): the rows every report and comparison lists.
ROWS = (
    ("retrieval", "precision@3", "Precision@3", "higher"),
    ("retrieval", "precision@6", "Precision@6", "higher"),
    ("retrieval", "recall@3", "Recall@3", "higher"),
    ("retrieval", "recall@6", "Recall@6", "higher"),
    ("retrieval", "mrr", "MRR", "higher"),
    ("influence", "harmful_exposure_rate", "Queries shown a stale/incorrect memory", "lower"),
    ("influence", "harmful_slot_share", "Retrieved slots holding a stale/incorrect memory", "lower"),
    ("influence", "conflict_loss_rate", "Conflicts where the old fact ranks first or alone", "lower"),
    ("store", "duplicate_share", "Stored memories that restate another", "lower"),
    ("store", "low_value_share", "Stored memories that are one-off observations", "lower"),
    ("store", "harmful_current_share", "Stored current memories that are stale/incorrect", "lower"),
    ("store", "near_duplicates_admitted", "Near-duplicates admitted", "lower"),
    ("store", "transient_rejected", "Transient candidates rejected", "higher"),
    ("injection", "mean_slots", "Memories injected per turn", "lower"),
    ("injection", "tokens_per_turn_mean", "Injected tokens per turn (mean)", "lower"),
    ("injection", "tokens_per_turn_p95", "Injected tokens per turn (p95)", "lower"),
    ("injection", "wasted_token_share", "Injected tokens not relevant to the task", "lower"),
    ("injection", "tokens_on_queries_without_relevant", "Tokens injected when nothing is relevant", "lower"),
    ("injection", "redundant_slot_share", "Slots repeating a memory already shown", "lower"),
    ("injection", "low_value_slot_share", "Slots holding a one-off observation", "lower"),
    ("injection", "model_calls_per_retrieval", "Model calls per retrieval", "lower"),
    ("recovery", "restart_identical_share", "Queries ranked identically after restart", "higher"),
)
MEASURED_ROWS = (
    ("latency_us", "median", "Retrieval latency, dataset store (median, us)"),
    ("latency_us", "p95", "Retrieval latency, dataset store (p95, us)"),
    ("latency_us", "scaled_median", "Retrieval latency, scaled store (median, us)"),
    ("latency_us", "scaled_p95", "Retrieval latency, scaled store (p95, us)"),
    ("storage_bytes", "per_record", "Database bytes per record, dataset store"),
    ("storage_bytes", "scaled_per_memory", "Database bytes per memory, scaled store"),
    ("storage_bytes", "seeded_on_disk", "Dataset store on disk, WAL included (bytes)"),
    ("storage_bytes", "scaled_on_disk", "Scaled store on disk, WAL included (bytes)"),
)


def fmt(value):
    if value is None:
        return "-"
    if isinstance(value, bool):
        return "yes" if value else "no"
    if isinstance(value, float):
        return f"{value:.4f}".rstrip("0").rstrip(".") if value != int(value) else f"{int(value)}"
    return str(value)


def render(results):
    """One run: every arm's metrics, then per-query detail for the first arm."""
    arms = results["arms"]
    names = list(arms)
    lines = [
        f"# Memory benchmark: {results['set']}",
        "",
        f"Commit `{results['commit']}`{' (dirty tree)' if results['dirty'] else ''}; "
        f"arms: {', '.join(f'index={n}' for n in names)}.",
        "",
        "| Metric | " + " | ".join(f"index={n}" for n in names) + " |",
        "|---|" + "---|" * len(names),
    ]
    for section, metric, label, better in ROWS:
        cells = [fmt(arms[n]["deterministic"][section].get(metric)) for n in names]
        lines.append(f"| {label} ({better} is better) | " + " | ".join(cells) + " |")
    for section, metric, label in MEASURED_ROWS:
        cells = [fmt(arms[n]["measured"][section].get(metric)) for n in names]
        lines.append(f"| {label} (machine-dependent) | " + " | ".join(cells) + " |")
    lines += ["", *_recovery(arms[names[0]]), "", *_queries(arms[names[0]], names[0])]
    if results.get("task_arm"):
        lines += ["", *render_task_arm(results["task_arm"])]
    return "\n".join(lines) + "\n"


def _recovery(arm):
    recovery = arm["deterministic"]["recovery"]
    lines = [
        "## Recovery",
        "",
        f"Restart: share of queries ranked identically {fmt(recovery['restart_identical_share'])}; "
        f"memories kept: {fmt(recovery['restart_memories_kept'])}.",
        "",
        "| Damage | Open | Retrieval | Memories recovered |",
        "|---|---|---|---|",
    ]
    for case, row in recovery["corruption"].items():
        lines.append(f"| {case} | {row['open']} | {row['retrieval']} | {fmt(row['recovered_share'])} |")
    return lines


def _queries(arm, name):
    queries = arm["queries"]
    k = max((int(key.split("@")[1]) for key in queries[0] if key.startswith("precision@")), default=0)
    lines = [
        f"## Queries (index={name})",
        "",
        f"| Query | P@{k} | R@{k} | Harmful | Tokens | Ranked |",
        "|---|---|---|---|---|---|",
    ]
    for q in queries:
        ranked = ", ".join(key.split("/", 1)[1] for key in q["ranked"]) or "-"
        lines.append(
            f"| {q['id']} | {fmt(q[f'precision@{k}'])} | {fmt(q[f'recall@{k}'])} | {q['harmful_slots']} | "
            f"{q['layer_tokens']} | {ranked} |"
        )
    return lines


def render_task_arm(task_arm):
    lines = [
        "## Task-success arm",
        "",
        f"Model command `{task_arm['command']}`, {task_arm['calls']} calls over {task_arm['tasks']} tasks.",
        "",
        "| Condition | Success |",
        "|---|---|",
    ]
    for condition, rate in task_arm["success"].items():
        lines.append(f"| {condition} | {fmt(rate)} |")
    lines += [
        "",
        f"Evidence (paired ablation, same tasks): retrieved - none = {fmt(task_arm['effect_retrieved'])}; "
        f"oracle - none = {fmt(task_arm['effect_oracle'])}.",
        f"Correlation only (observational): success with a relevant memory retrieved = "
        f"{fmt(task_arm['success_when_relevant_retrieved'])}, without = {fmt(task_arm['success_when_not'])}.",
    ]
    return lines


def compare(before, after):
    """Arm by arm, each metric before and after, with the change."""
    lines = [
        f"# Memory benchmark comparison: {before['set']}",
        "",
        f"Before `{before['commit']}`, after `{after['commit']}`.",
    ]
    for arm in sorted(set(before["arms"]) & set(after["arms"])):
        a, b = before["arms"][arm], after["arms"][arm]
        lines += ["", f"## index={arm}", "", "| Metric | Before | After | Change |", "|---|---|---|---|"]
        for section, metric, label, better in ROWS:
            lines.append(_compare_row(f"{label} ({better} is better)",
                                      a["deterministic"][section].get(metric), b["deterministic"][section].get(metric)))
        for section, metric, label in MEASURED_ROWS:
            lines.append(_compare_row(f"{label} (machine-dependent)",
                                      a["measured"][section].get(metric), b["measured"][section].get(metric)))
    return "\n".join(lines) + "\n"


def _compare_row(label, x, y):
    numeric = all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in (x, y))
    change = fmt(round(y - x, 4)) if numeric else "-"
    return f"| {label} | {fmt(x)} | {fmt(y)} | {change} |"
