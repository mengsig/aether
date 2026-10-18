"""One release-bench trial: a fresh workspace, one arm's harness, hidden grading and priced usage.

Every trial owns its directory: TMPDIR, XDG cache/state, logs and (for Aether
arms) its own daemon, config, data and socket; its workspace lives under the
run's work root, outside the repository that holds the reference solutions.
Nothing in a trial touches another trial's files, so trials run side by side.
Every process of a trial carries a marker, so its spend guard can stop them all.
"""
import os
import re
import shutil
import tempfile
import time
import traceback
from pathlib import Path

from harness_bench import run_aether_trial, run_native_trial, save_json
from harness_bench_report import trial_summary
from harness_bench_reprice import rebuild_trial_usage
from release_bench_archon import run_archon_trial
from release_bench_grade import grade
from release_bench_spend import (
    ALLOWANCE_USD,
    MARKER,
    AetherStoreProbe,
    ClaudeStreamProbe,
    CodexRolloutProbe,
    SpendGuard,
    price_of,
)
from release_bench_tasks import materialize

TRIAL_TIMEOUT_SECONDS = 600
# Harnesses bind Unix sockets under TMPDIR (108-byte limit), so a trial's TMPDIR must stay short.
SHORT_TMP_ROOT = os.environ.get("RELEASE_BENCH_TMP_ROOT", "/var/tmp")
SHORT_TMP_MAX = 40
# Failures the harness under test is answerable for: a counted, failed trial on old, new and archon arms.
HARNESS_FAILURE_KEYS = ("telemetry_error", "daemon_cleanup_error", "cleanup_error")
# A provider or network outage named in a failed run's error text says nothing about the harness.
PROVIDER_OUTAGE = re.compile(r"overloaded|rate.?limit|too many requests|usage limit|service unavailable"
                             r"|bad gateway|gateway time-?out|connection reset|\b(429|502|503|504|529)\b",
                             re.IGNORECASE)


def trial_name(task_id, arm_key, repeat):
    assert "/" in arm_key and repeat >= 0, "a trial is named by a task, an arm key and a repeat"
    return f"{task_id}--{arm_key.replace('/', '-')}--r{repeat}"


def trial_env(trial_dir, marker):
    """Private TMPDIR (short, outside the trial directory) and XDG cache/state for one trial, plus its marker."""
    paths = {"XDG_CACHE_HOME": "xdg-cache", "XDG_STATE_HOME": "xdg-state"}
    env = {MARKER: marker, "TMPDIR": tempfile.mkdtemp(prefix="rb-", dir=SHORT_TMP_ROOT)}
    for name, relative in paths.items():
        path = Path(trial_dir, relative).resolve()
        path.mkdir(parents=True, exist_ok=False)
        env[name] = str(path)
    assert len(env["TMPDIR"]) <= SHORT_TMP_MAX, f"TMPDIR {env['TMPDIR']} leaves too little room for a socket"
    assert all(Path(env[name]).is_dir() for name in (*paths, "TMPDIR")), "every private directory exists"
    return env


def _guard(arm, trial_dir, context, marker):
    """The spend guard of one trial, reading whatever its harness writes as it runs."""
    model, harness = arm["model"], arm["harness"]
    price = price_of(context["price_manifest"]["price_table"], model)
    if harness == "native":
        stdout = Path(trial_dir, "native-stdout.jsonl")
        probe = ClaudeStreamProbe(stdout, price) if model == "haiku" else CodexRolloutProbe(stdout, price)
    elif harness == "archon":
        probe = AetherStoreProbe(Path(trial_dir, "archon", "data", "aether"), price)
    else:
        probe = AetherStoreProbe(Path(trial_dir, "data"), price)
    return SpendGuard(probe, ALLOWANCE_USD[model], marker)


def _solve(arm, task_text, workspace, trial_dir, context, result, marker):
    env = trial_env(trial_dir, marker)
    try:
        _solve_in(arm, task_text, workspace, trial_dir, context, result, marker, env)
    finally:
        # The harness's own temporary files are not evidence; a failure to remove them is reported.
        try:
            shutil.rmtree(env["TMPDIR"])
        except OSError as error:
            result["tmpdir_cleanup_error"] = f"{env['TMPDIR']}: {error}"


def _solve_in(arm, task_text, workspace, trial_dir, context, result, marker, env):
    condition, timeout = arm["condition"], context["timeout"]
    with _guard(arm, trial_dir, context, marker) as guard:
        if arm["harness"] == "native":
            run_native_trial(condition, task_text, workspace, timeout, trial_dir, result, extra_env=env)
        elif arm["harness"] == "archon":
            price = price_of(context["price_manifest"]["price_table"], arm["model"])
            run_archon_trial(context["archon_bin"], context["binaries"]["new"], condition, task_text, workspace,
                             timeout, trial_dir, result, env, price)
        else:
            binary = context["binaries"][arm["binary_role"]]
            run_aether_trial(binary, condition, task_text, workspace, timeout, trial_dir, context["real_claude"],
                             result, extra_env=env)
    result["spend_guard"] = {"allowance_usd": guard.allowance_usd, "peak_usd": round(guard.peak_usd, 6)}
    if guard.tripped:
        result["spend_stop"] = guard.tripped
        result["spend_unobserved"] = guard.unobserved


def _price(row, arm, context):
    """(cost in USD or None, tokens or None) of a finished trial, priced on the run's one price table."""
    if arm["harness"] == "archon":
        usage = row.get("archon_usage") or {}
        return usage.get("api_equivalent_usd"), usage.get("total_tokens")
    rebuild_trial_usage(row, context["price_manifest"])
    summary = trial_summary(row)
    if summary["cost_priced"] and arm["harness"] == "native" and arm["model"] == "luna":
        return _with_codex_descendants(row, arm, context, summary)
    if summary["cost_priced"]:
        return summary["api_equivalent_usd"], summary["tokens"]
    if arm["harness"] in ("old", "new"):
        return _price_from_store(row, arm, context), summary["tokens"]
    return None, summary["tokens"]


def _with_codex_descendants(row, arm, context, summary):
    """A native Codex trial's cost and tokens with its subagent threads, whose usage only their rollouts hold.

    Without the root's rollout the subagents cannot be counted, so the trial stays unpriced."""
    assert summary["api_equivalent_usd"] is not None, "the root's own usage is priced"
    probe = CodexRolloutProbe(row["raw_logs"]["stdout"], price_of(context["price_manifest"]["price_table"],
                                                                  arm["model"]))
    if probe() is None:
        row["price_note"] = "the root's Codex rollout was not found, so its subagents' spend is unknown"
        return None, summary["tokens"]
    children = probe.descendants()
    row["descendant_count"] = len(children)
    row["descendant_usage"] = {thread: {"api_equivalent_usd": usd, "total_tokens": tokens}
                               for thread, (usd, tokens) in children.items()}
    cost = summary["api_equivalent_usd"] + sum(usd for usd, _ in children.values())
    tokens = (summary["tokens"] or 0) + sum(tokens for _, tokens in children.values())
    assert cost >= summary["api_equivalent_usd"], "subagents add spend, never remove it"
    return cost, tokens


def _price_from_store(row, arm, context):
    """The trial's cost from its Aether store, when every unpriced unit recorded no tokens, else None.

    A turn that failed before any model call (a sandbox that did not start, say) or
    ended without one spent nothing; it must not unprice the trial."""
    units = (row.get("usage_report") or {}).get("units") or []
    unpriced = [unit for unit in units if unit["usage"].get("api_equivalent_cost_usd") is None]
    if not unpriced or any(unit["usage"].get("total_tokens") for unit in unpriced):
        return None
    probe = AetherStoreProbe(Path(row["trial_dir"], "data"), price_of(context["price_manifest"]["price_table"],
                                                                       arm["model"]))
    cost = probe()
    if cost is None:
        return None
    priced = {unit["agent_id"] for unit in units if unit["usage"].get("api_equivalent_cost_usd") is not None}
    assert cost >= 0 and priced <= set(probe.totals), "every agent with priced usage is in the store"
    assert set(probe.totals) <= {unit["agent_id"] for unit in units}, "the store holds no agent the report lacks"
    row["price_note"] = f"{len(unpriced)} unit(s) recorded no tokens and are priced at zero"
    return cost


def _model_output(row):
    usage = (row.get("usage_report") or {}).get("usage") or {}
    return bool(usage.get("output_tokens"))


def _outage(row):
    """A provider outage named in a failed run's error text, or None."""
    if row.get("workflow_status") == "completed":
        return None
    texts = [str(row.get("workflow_error") or "")]
    stderr = (row.get("raw_logs") or {}).get("stderr")
    if stderr and os.path.exists(stderr):
        with open(stderr, encoding="utf-8", errors="replace") as stream:
            texts.append(stream.read()[-4000:])
    match = next((found for text in texts for found in [PROVIDER_OUTAGE.search(text)] if found), None)
    return None if match is None else f"provider: {match.group(0)}"


def infra_error(row, harness):
    """Why a trial says nothing about the harness under test, or None when it counts.

    Only the bench's own steps, provider outages and (on the native drift
    reference) a CLI that never produced output are excluded. A failure of an
    Aether or Archon arm is the harness's own: see `harness_failure`."""
    assert harness in ("native", "old", "new", "archon"), f"unknown harness {harness!r}"
    if row.get("bench_error"):
        return f"bench: {str(row['bench_error'])[:300]}"
    if row.get("spend_stop"):
        # The native reference cannot be blamed for a spend source the bench could not read.
        return f"bench: {row['spend_stop']}" if harness == "native" and row.get("spend_unobserved") else None
    outage = _outage(row)
    if outage:
        return outage
    if harness != "native":
        return None
    if row.get("cleanup_error"):
        return f"cleanup_error: {str(row['cleanup_error'])[:300]}"
    if row.get("workflow_status") == "failed" and not _model_output(row):
        return f"no model output: {str(row.get('workflow_error') or row.get('exit_code'))[:300]}"
    return None


def harness_failure(row, harness):
    """Why the harness itself failed a trial (a counted failure whatever the grade), or None."""
    if row.get("spend_stop"):
        return f"spend guard: {row['spend_stop']}"
    if harness == "native":
        return None
    for key in HARNESS_FAILURE_KEYS:
        if row.get(key):
            return f"{key}: {str(row[key])[:300]}"
    if row.get("blocked"):
        return "blocked: the root idled after a tool error"
    if row.get("workflow_status") in ("failed", "timeout") and not _model_output(row):
        return f"no model output: {str(row.get('workflow_error') or row.get('exit_code'))[:300]}"
    return None


def record(spec, arm, repeat, row, grade_result, cost, tokens):
    """The compact line results.jsonl keeps per trial; the full row stays in the trial directory."""
    assert grade_result["total"] == spec["checks"], "a grade covers every hidden check of its task"
    error = infra_error(row, arm["harness"])
    failure = None if error else harness_failure(row, arm["harness"])
    passed = error is None and failure is None and grade_result["passed"] == grade_result["total"]
    return {"task": spec["id"], "family": spec["family"], "template": spec["template"], "arm": arm["key"],
            "model": arm["model"], "harness": arm["harness"], "repeat": repeat, "passed": passed,
            "checks_passed": grade_result["passed"], "checks_total": grade_result["total"],
            "fraction": grade_result["passed"] / grade_result["total"], "grade_error": grade_result["error"],
            "cost_usd": cost, "tokens": tokens, "seconds": row.get("solve_seconds"),
            "workflow": row.get("workflow_status"), "infra_error": error, "harness_failure": failure,
            "children": _children(row), "trial_dir": row["trial_dir"],
            "started_at": row["started_at"]}


def _children(row):
    """Agents the trial's root started: Aether's descendants, or Claude Code's Task subagents."""
    if row.get("descendant_count") is not None:
        return row["descendant_count"]
    tools = (row.get("usage_report") or {}).get("tool_calls") or {}
    return sum(count for name, count in tools.items() if name in ("Task", "Agent"))


def _fresh_path(path):
    """`path`, after moving an abandoned attempt there aside (kept as evidence) when a resume finds one."""
    path = Path(path)
    if not path.exists():
        return path
    attempt = 1
    while path.with_name(f"{path.name}.abandoned-{attempt}").exists():
        attempt += 1
    path.rename(path.with_name(f"{path.name}.abandoned-{attempt}"))
    assert not path.exists(), "the abandoned attempt moved aside"
    return path


def run_release_trial(spec, arm, repeat, context):
    """Run, grade and price one (task, arm, repeat); a bench crash becomes an infra-error record."""
    name = trial_name(spec["id"], arm["key"], repeat)
    trial_dir = _fresh_path(Path(context["run_dir"], "trials", name).resolve())
    trial_dir.mkdir(parents=True, exist_ok=False)
    workspace = Path(context["work_dir"], name, "workspace")
    _fresh_path(workspace.parent)
    row = {"condition": "archon" if arm["harness"] == "archon" else _condition_name(arm),
           "case": spec["id"], "run": repeat, "trial_dir": str(trial_dir), "workspace": str(workspace),
           "requested_treatment": arm["condition"], "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    grade_result = {"passed": 0, "total": spec["checks"], "failed": ["*"], "error": "not graded"}
    cost = tokens = None
    try:
        materialize(spec, workspace)
        task_text = spec["prompt"] + arm["condition"]["suffix"]
        _solve(arm, task_text, workspace, str(trial_dir), context, row, f"{context['run_dir']}::{name}")
        grade_result = grade(workspace, spec["hidden"], spec["checks"], trial_dir / "grade")
    except Exception as error:
        row["bench_error"] = f"{type(error).__name__}: {error}"
        row["bench_traceback"] = traceback.format_exc()
    try:
        cost, tokens = _price(row, arm, context)
    except Exception as error:
        # A trial whose usage cannot be priced stays unpriced; the cost gate counts unpriced trials.
        row["price_error"] = f"{type(error).__name__}: {error}"
    row["grade"] = grade_result
    save_json(str(trial_dir / "result.json"), row)
    return record(spec, arm, repeat, row, grade_result, cost, tokens)


def _condition_name(arm):
    """The harness_bench cell an arm runs, which its usage accounting keys on."""
    kind = arm["condition"]["kind"]
    return f"{'native' if kind.startswith('native-') else 'aether-single'}-{arm['model']}-low"
