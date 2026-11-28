"""Archon parity and the failure digest of a release-bench run.

Parity holds Archon to the operator's standing rule: on every model tier, the
Archon candidate (`archon-new`) passes at least as often as the plain Aether
candidate (`new`) on the same (task, repeat) cells.

The failure digest says, per arm, why its trials did not pass, by category
only. It names no task, family, prompt or check, so it can be handed to the
people fixing Aether or Archon without showing them the set.
"""
import re
from collections import Counter

from release_bench_stats import paired_cells

REVIEW_CAP = re.compile(r"review round cap", re.IGNORECASE)
# An Archon stage that hands the work back (ESCALATE:) or names a missing prerequisite (BLOCKED:).
HANDED_BACK = re.compile(r"\b(ESCALATE|BLOCKED):")
DIGEST_CATEGORIES = (
    "excluded: infrastructure or provider outage",
    "spend allowance reached",
    "timeout",
    "review rounds exhausted",
    "escalated or blocked",
    "harness error (crash, no model output, cleanup)",
    "run ended in error, its workspace graded",
    "result not gradable",
    "wrong result: no hidden check passes",
    "wrong result: under half the hidden checks pass",
    "wrong result: half or more of the hidden checks pass",
)


def parity(records, model):
    """{"status": MET, MISSED or UNKNOWN, rates and the tasks `new` passed more often than `archon-new`}.

    Compared on the (task, repeat) cells both arms counted, so an outage on one arm is no loss."""
    new_arm, archon_arm = f"{model}/new", f"{model}/archon-new"
    cells = paired_cells(records, new_arm, archon_arm)
    pairs = [pair for task_cells in cells.values() for pair in task_cells]
    result = {"model": model, "pairs": len(pairs), "new": None, "archon_new": None, "behind": []}
    if not pairs:
        return {**result, "status": "UNKNOWN"}
    result["new"] = sum(new["passed"] for new, _ in pairs) / len(pairs)
    result["archon_new"] = sum(archon["passed"] for _, archon in pairs) / len(pairs)
    result["behind"] = sorted(task for task, task_cells in cells.items()
                              if sum(new["passed"] for new, _ in task_cells)
                              > sum(archon["passed"] for _, archon in task_cells))
    met = result["archon_new"] >= result["new"]
    assert met or result["behind"], "a parity miss is carried by at least one task"
    return {**result, "status": "MET" if met else "MISSED"}


def parity_line(result):
    """One line per tier: PARITY MET, PARITY MISSED with the tasks, or why it is unknown."""
    model = result["model"]
    if result["status"] == "UNKNOWN":
        return f"- **{model}: PARITY UNKNOWN**: no (task, repeat) cell was counted on both new and archon-new"
    rates = (f"archon-new {result['archon_new']:.0%} vs new {result['new']:.0%} "
             f"over {result['pairs']} paired trials")
    if result["status"] == "MET":
        return f"- **{model}: PARITY MET**: {rates}"
    return f"- **{model}: PARITY MISSED**: {rates}; new passed more often on: {', '.join(result['behind'])}"


def failure_category(item):
    """The digest category of a trial that did not pass, or None for a pass."""
    if item["infra_error"]:
        return DIGEST_CATEGORIES[0]
    if item["passed"]:
        return None
    failure = item.get("harness_failure") or ""
    if failure.startswith("spend guard"):
        return DIGEST_CATEGORIES[1]
    if item.get("workflow") == "timeout":
        return DIGEST_CATEGORIES[2]
    error = item.get("workflow_error") or ""
    if REVIEW_CAP.search(error):
        return DIGEST_CATEGORIES[3]
    if HANDED_BACK.search(error):
        return DIGEST_CATEGORIES[4]
    if failure:
        return DIGEST_CATEGORIES[5]
    if item.get("workflow") == "failed":
        return DIGEST_CATEGORIES[6]
    if item.get("grade_error"):
        return DIGEST_CATEGORIES[7]
    return _wrong_result(item)


def _wrong_result(item):
    total, passed = item["checks_total"], item["checks_passed"]
    assert total > 0 and 0 <= passed <= total, "a graded trial has checks"
    if passed == 0:
        return DIGEST_CATEGORIES[8]
    return DIGEST_CATEGORIES[9] if passed * 2 < total else DIGEST_CATEGORIES[10]


def failure_digest(records, arms):
    """{arm: {"trials", "passed", "categories": Counter}} for every planned arm, records or not."""
    digest = {arm: {"trials": 0, "passed": 0, "categories": Counter()} for arm in arms}
    for item in records:
        row = digest[item["arm"]]
        row["trials"] += 1
        category = failure_category(item)
        if category is None:
            row["passed"] += 1
        else:
            row["categories"][category] += 1
    assert all(row["passed"] + sum(row["categories"].values()) == row["trials"] for row in digest.values()), \
        "every trial is a pass or in exactly one category"
    return digest


def digest_lines(digest):
    """The digest as Markdown: arm, trials, passes and a count per category. No task content."""
    lines = ["| Arm | Trials | Passed | Why the others did not pass |", "|---|---:|---:|---|"]
    for arm, row in sorted(digest.items()):
        reasons = "; ".join(f"{category} {row['categories'][category]}" for category in DIGEST_CATEGORIES
                            if row["categories"][category]) or "–"
        lines.append(f"| {arm} | {row['trials']} | {row['passed']} | {reasons} |")
    return lines


def digest_document(digest, provenance):
    """A stand-alone digest for the people fixing a harness: categories per arm, nothing about the tasks."""
    assert all("/" in arm for arm in digest), "digest rows are arm keys"
    return "\n".join(["# Release bench failure digest", "",
                      "Counts per arm of why trials did not pass. It names no task, prompt or check.", "",
                      *provenance, "", *digest_lines(digest), ""])

