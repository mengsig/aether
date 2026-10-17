"""One release-bench trial: a fresh workspace, one arm's harness, hidden grading and priced usage.

Every trial owns its directory: TMPDIR, XDG cache/state, logs and (for Aether
arms) its own daemon, config, data and socket; its workspace lives under the
run's work root, outside the repository that holds the reference solutions.
Nothing in a trial touches another trial's files, so trials run side by side.
"""
import json
import os
import time
import traceback
from pathlib import Path

from harness_bench import run_aether_trial, run_native_trial, save_json
from harness_bench_config import sanitized_native_env
from harness_bench_process import run_subprocess_capturing
from harness_bench_report import trial_summary
from harness_bench_reprice import rebuild_trial_usage
from release_bench_grade import grade
from release_bench_tasks import materialize

TRIAL_TIMEOUT_SECONDS = 600
# Errors of the bench or the host, not of the harness under test: such a trial is excluded, not failed.
INFRA_KEYS = ("harness_error", "telemetry_error", "daemon_cleanup_error", "cleanup_error")


def trial_name(task_id, arm_key, repeat):
    assert "/" in arm_key and repeat >= 0, "a trial is named by a task, an arm key and a repeat"
    return f"{task_id}--{arm_key.replace('/', '-')}--r{repeat}"


def trial_env(trial_dir):
    """Private TMPDIR and XDG cache/state for every process of one trial."""
    paths = {"TMPDIR": "tmp", "XDG_CACHE_HOME": "xdg-cache", "XDG_STATE_HOME": "xdg-state"}
    env = {}
    for name, relative in paths.items():
        path = Path(trial_dir, relative).resolve()
        path.mkdir(parents=True, exist_ok=False)
        env[name] = str(path)
    assert all(Path(path).is_dir() for path in env.values()), "every private directory exists"
    return env


def run_adapter_trial(command, condition, task_text, workspace, timeout, trial_dir, result, extra_env):
    """Run an external harness (the Archon arm) through the adapter contract in docs/release-bench.md."""
    assert Path(command).is_file() and os.access(command, os.X_OK), f"adapter is not executable: {command}"
    prompt_path, usage_path = Path(trial_dir, "prompt.txt"), Path(trial_dir, "adapter-usage.json")
    prompt_path.write_text(task_text, encoding="utf-8")
    env, stripped = sanitized_native_env()
    env.update(extra_env, BENCH_PROMPT_FILE=str(prompt_path), BENCH_WORKSPACE=str(workspace),
               BENCH_MODEL=condition["model"], BENCH_EFFORT=condition["effort"], BENCH_USAGE_FILE=str(usage_path))
    stdout_path, stderr_path = Path(trial_dir, "adapter-stdout.log"), Path(trial_dir, "adapter-stderr.log")
    info = run_subprocess_capturing([command], str(workspace), timeout, stdout_path, stderr_path, env=env)
    result.update(env_isolation={"stripped_vars": stripped}, command=[command], exit_code=info["exit_code"],
                  timed_out=info["timed_out"], cleanup_error=info["cleanup_error"], solve_seconds=info["seconds"],
                  workflow_status="timeout" if info["timed_out"] else
                  ("completed" if info["exit_code"] == 0 else "failed"))
    if usage_path.exists():
        usage = json.loads(usage_path.read_text())
        cost = usage.get("api_equivalent_usd")
        if cost is not None and (not isinstance(cost, (int, float)) or cost < 0):
            raise ValueError(f"adapter reported an impossible cost: {cost!r}")
        result["adapter_usage"] = usage


def _solve(arm, task_text, workspace, trial_dir, context, result):
    condition, env = arm["condition"], trial_env(trial_dir)
    timeout = context["timeout"]
    if arm["harness"] == "native":
        run_native_trial(condition, task_text, workspace, timeout, trial_dir, result, extra_env=env)
    elif arm["harness"] == "archon":
        run_adapter_trial(context["archon_bin"], condition, task_text, workspace, timeout, trial_dir, result, env)
    else:
        binary = context["binaries"][arm["binary_role"]]
        run_aether_trial(binary, condition, task_text, workspace, timeout, trial_dir, context["real_claude"],
                         result, extra_env=env)


def _price(row, arm, context):
    """(cost in USD or None, tokens or None) of a finished trial, priced on the run's one price table."""
    if arm["harness"] == "archon":
        usage = row.get("adapter_usage") or {}
        return usage.get("api_equivalent_usd"), usage.get("total_tokens")
    rebuild_trial_usage(row, context["price_manifest"])
    summary = trial_summary(row)
    cost = summary["api_equivalent_usd"] if summary["cost_priced"] else None
    return cost, summary["tokens"]


def infra_error(row):
    """Why a trial says nothing about the harness under test, or None when it counts."""
    for key in INFRA_KEYS:
        if row.get(key):
            return f"{key}: {str(row[key])[:300]}"
    if row.get("blocked"):
        return "blocked by the host"
    usage = (row.get("usage_report") or {}).get("usage") or {}
    if row.get("workflow_status") == "failed" and not usage.get("output_tokens"):
        return f"no model output: {str(row.get('workflow_error') or row.get('exit_code'))[:300]}"
    return None


def record(spec, arm, repeat, row, grade_result, cost, tokens):
    """The compact line results.jsonl keeps per trial; the full row stays in the trial directory."""
    assert grade_result["total"] == spec["checks"], "a grade covers every hidden check of its task"
    error = infra_error(row)
    passed = error is None and grade_result["passed"] == grade_result["total"]
    return {"task": spec["id"], "family": spec["family"], "template": spec["template"], "arm": arm["key"],
            "model": arm["model"], "harness": arm["harness"], "repeat": repeat, "passed": passed,
            "checks_passed": grade_result["passed"], "checks_total": grade_result["total"],
            "fraction": grade_result["passed"] / grade_result["total"], "grade_error": grade_result["error"],
            "cost_usd": cost, "tokens": tokens, "seconds": row.get("solve_seconds"),
            "workflow": row.get("workflow_status"), "infra_error": error, "trial_dir": row["trial_dir"],
            "started_at": row["started_at"]}


def run_release_trial(spec, arm, repeat, context):
    """Run, grade and price one (task, arm, repeat); a bench crash becomes an infra-error record."""
    name = trial_name(spec["id"], arm["key"], repeat)
    trial_dir = Path(context["run_dir"], "trials", name).resolve()
    trial_dir.mkdir(parents=True, exist_ok=False)
    workspace = Path(context["work_dir"], name, "workspace")
    row = {"condition": arm["condition"].get("kind") if arm["harness"] == "archon" else _condition_name(arm),
           "case": spec["id"], "run": repeat, "trial_dir": str(trial_dir), "workspace": str(workspace),
           "requested_treatment": arm["condition"], "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    grade_result = {"passed": 0, "total": spec["checks"], "failed": ["*"], "error": "not graded"}
    cost = tokens = None
    try:
        materialize(spec, workspace)
        task_text = spec["prompt"] + arm["condition"]["suffix"]
        _solve(arm, task_text, workspace, str(trial_dir), context, row)
        grade_result = grade(workspace, spec["hidden"], spec["checks"], trial_dir / "grade")
        cost, tokens = _price(row, arm, context)
    except Exception as error:
        row["harness_error"] = f"{type(error).__name__}: {error}"
        row["harness_traceback"] = traceback.format_exc()
    row["grade"] = grade_result
    save_json(str(trial_dir / "result.json"), row)
    return record(spec, arm, repeat, row, grade_result, cost, tokens)


def _condition_name(arm):
    """The harness_bench cell an arm runs, which its usage accounting keys on."""
    kind = arm["condition"]["kind"]
    return f"{'native' if kind.startswith('native-') else 'aether-single'}-{arm['model']}-low"
