"""The Archon arm: one task, queued on a private Archon whose own Aether is the candidate binary.

Each trial gets its own Archon config, data and socket. Archon runs in
dedicated mode, so it starts its own aetherd from the first `aether` on PATH,
a link to the pinned candidate binary, with its config seeded from an
operator config that holds only the trial's cap on model turns at once:
nothing from the machine's Archon or Aether is read or reached. The task goes
in as one `direct` pipeline item, on the arm's model at low effort with
Archon's own budget cap. Archon ships it locally, and the
newest run branch holding commits becomes the graded workspace. Usage is read
from every agent in the private Aether store and priced on the run's table.
"""
import json
import os
import subprocess
import time
from pathlib import Path

from harness_bench import isolated_socket_path, remove_socket_dir
from harness_bench_config import scheduler_config_toml, strip_live_routing_env
from release_bench_spend import AetherStoreProbe

PIPELINE = "direct"
STAGE = "implement"
# `archon queue wait` exit codes (archon queue wait --help).
WAIT_STATUS = {0: "completed", 3: "failed", 4: "timeout"}
COMMAND_SECONDS = 120
WAIT_SLACK_SECONDS = 60


class ArchonError(RuntimeError):
    """An `archon` command failed; its output is in the message."""


def archon_layout(trial_dir):
    """The private directories, socket and PATH entry of one trial's Archon."""
    root = Path(trial_dir, "archon").resolve()
    aether_socket, socket_dir = isolated_socket_path(str(root))
    layout = {"root": root, "config": root / "config", "data": root / "data", "bin": root / "bin",
              "operator": root / "operator-aether", "socket": Path(aether_socket).with_name("archond.sock"),
              "socket_dir": socket_dir}
    # Archon's dedicated Aether lives at <data>/aether and listens beside archond.
    layout["aether_data"] = layout["data"] / "aether"
    assert layout["socket"].parent == Path(aether_socket).parent, "aetherd's socket sits beside archond's"
    assert len(str(layout["socket"]).encode()) == len(aether_socket.encode()), "both socket names fit the limit"
    return layout


def config_toml(model_ref):
    """Archon's config: dedicated Aether, the arm's model only, at low effort, budgets enforced, nothing extra."""
    assert "/" in model_ref, "an Archon model names its engine and variant"
    model = json.dumps(model_ref)
    return "\n".join([
        "[aether]", 'mode = "dedicated"', "",
        "[orchestrator]", f"model = {model}", 'effort = "low"', "wake_on_success = false", "",
        "[models]", f"allow = [{model}]", 'max_effort = "low"', "",
        "[limits]", "budgets_enabled = true", "",
        "[lavish]", 'enabled = "off"', "",
        "[worktrees]", "keep_on_success = true", ""])


def prepare(layout, aether_binary, model_ref, max_concurrent_turns):
    """Write the trial's Archon config and put the candidate binary first on its PATH.

    The operator config Archon seeds its Aether from holds only the trial's turn cap."""
    aether_binary = Path(aether_binary).resolve()
    assert aether_binary.is_file(), f"the candidate aether binary exists: {aether_binary}"
    for key in ("config", "data", "bin", "operator"):
        layout[key].mkdir(parents=True, exist_ok=False)
    (layout["bin"] / "aether").symlink_to(aether_binary)
    (layout["operator"] / "config.toml").write_text(scheduler_config_toml(max_concurrent_turns), encoding="utf-8")
    (layout["config"] / "config.toml").write_text(config_toml(model_ref), encoding="utf-8")
    assert (layout["bin"] / "aether").resolve() == aether_binary, "PATH's aether is the candidate"


def archon_env(layout, extra_env):
    """This process's environment without any route to a live Aether, plus the trial's Archon."""
    env = dict(os.environ)
    stripped = strip_live_routing_env(env)
    env.update(extra_env)
    env.update(ARCHON_CONFIG_DIR=str(layout["config"]), ARCHON_DATA_DIR=str(layout["data"]),
               ARCHON_SOCKET=str(layout["socket"]), AETHER_CONFIG_DIR=str(layout["operator"]),
               PATH=f"{layout['bin']}{os.pathsep}{env.get('PATH', '')}")
    assert not any(key.startswith("AETHER_") and key != "AETHER_CONFIG_DIR" for key in env), \
        "the trial's Archon finds its Aether only through its own directories"
    return env, stripped


def archon(archon_bin, env, args, log, timeout=COMMAND_SECONDS, ok=(0,)):
    """Run one `archon` command, logging it; returns (exit code, stdout)."""
    started = time.monotonic()
    done = subprocess.run([str(archon_bin), *args], env=env, capture_output=True, text=True, timeout=timeout,
                          check=False, stdin=subprocess.DEVNULL)
    log.append({"args": args, "exit_code": done.returncode, "seconds": round(time.monotonic() - started, 3),
                "stderr": done.stderr[-2000:]})
    if done.returncode not in ok:
        raise ArchonError(f"archon {' '.join(args[:2])} exited {done.returncode}: "
                          f"{(done.stderr or done.stdout).strip()[-1000:]}")
    return done.returncode, done.stdout


def _json(text, what):
    try:
        value = json.loads(text)
    except ValueError as error:
        raise ArchonError(f"{what} printed no JSON: {text[-500:]!r}") from error
    if not isinstance(value, dict):
        raise ArchonError(f"{what} printed {type(value).__name__}, not an object")
    return value


def enqueue(archon_bin, env, log, condition, task_text, workspace, budget_usd, layout):
    """Register the workspace and queue the task on it; returns the item id."""
    base = _git(workspace, "symbolic-ref", "--short", "HEAD")
    archon(archon_bin, env, ["repo", "add", str(workspace), "--name", "task", "--ship", "local", "--base", base,
                             "--json"], log)
    brief, plan = layout["root"] / "brief.md", layout["root"] / "model-plan.json"
    brief.write_text(task_text, encoding="utf-8")
    plan.write_text(json.dumps({STAGE: [{"model": condition["model"], "effort": condition["effort"]}]}),
                    encoding="utf-8")
    _, out = archon(archon_bin, env, [
        "queue", "add", "--json", "--title", "release-bench task", "--repo", "task", "--pipeline", PIPELINE,
        "--brief-file", str(brief), "--model-plan-file", str(plan), "--budget-usd", f"{budget_usd:.2f}",
        "--review", "none", "--report", "off", "--proposal", "off", "--merge", "manual"], log)
    added = _json(out, "archon queue add")
    item = added.get("id") or (added.get("item") or {}).get("id")
    if not isinstance(item, str) or not item.startswith("qi_"):
        raise ArchonError(f"archon queue add named no item: {out[-500:]!r}")
    return item


def wait(archon_bin, env, log, item, timeout):
    """Wait for the item to settle; a timed-out item is cancelled. Returns the workflow status."""
    assert timeout > 0, "a trial has a positive timeout"
    code, _ = archon(archon_bin, env, ["queue", "wait", item, "--timeout", f"{timeout}s", "--json"], log,
                     timeout=timeout + WAIT_SLACK_SECONDS, ok=tuple(WAIT_STATUS))
    status = WAIT_STATUS[code]
    if status == "timeout":
        archon(archon_bin, env, ["queue", "cancel", item, "--json"], log)
    return status


def item_detail(archon_bin, env, log, item, trial_dir):
    """The item with its runs, kept beside the trial's result."""
    _, out = archon(archon_bin, env, ["queue", "show", item, "--json"], log)
    detail = _json(out, "archon queue show")
    Path(trial_dir, "archon-item.json").write_text(json.dumps(detail, indent=2), encoding="utf-8")
    return detail


def result_ref(workspace, detail):
    """The newest run branch holding commits past the starting commit, else the starting commit itself."""
    start = _git(workspace, "rev-parse", "HEAD")
    runs = [run for run in detail.get("runs") or [] if isinstance(run, dict) and run.get("branch")]
    for run in sorted(runs, key=lambda run: str(run.get("started_at")), reverse=True):
        known = subprocess.run(["git", "-C", str(workspace), "rev-parse", "--verify", "--quiet",
                                f"refs/heads/{run['branch']}"], capture_output=True, text=True, check=False)
        if known.returncode == 0 and _git(workspace, "rev-list", "--count", f"{start}..{run['branch']}") != "0":
            return run["branch"]
    return start


def checkout(workspace, ref):
    """Make the workspace hold `ref`'s tree for grading."""
    _git(workspace, "reset", "-q", "--hard", ref)
    assert _git(workspace, "rev-parse", "HEAD") == _git(workspace, "rev-parse", ref), "the result is checked out"


def store_usage(data_dir, price):
    """Priced usage of every agent the trial's Aether ran: the orchestrator's and the stage's."""
    probe = AetherStoreProbe(data_dir, price)
    cost = probe()
    if cost is None:
        return {"api_equivalent_usd": None, "total_tokens": None, "agents": 0}
    tokens = probe.total_tokens()
    assert cost >= 0 and tokens >= 0, "usage is nonnegative"
    return {"api_equivalent_usd": round(cost, 6), "total_tokens": tokens, "agents": len(probe.totals),
            "output_tokens": probe.output_tokens()}


def _git(workspace, *args):
    done = subprocess.run(["git", "-C", str(workspace), *args], capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise ArchonError(f"git {args[0]} in {workspace} failed: {done.stderr.strip()[-500:]}")
    return done.stdout.strip()


def _session(archon_bin, env, log, condition, task_text, workspace, timeout, trial_dir, result, layout):
    archon(archon_bin, env, ["setup", "--json"], log)
    archon(archon_bin, env, ["daemon", "run", "--detach", "--json"], log)
    item = enqueue(archon_bin, env, log, condition, task_text, workspace, condition["budget_usd"], layout)
    result["archon_item"] = item
    status = wait(archon_bin, env, log, item, timeout)
    result["workflow_status"] = status
    detail = item_detail(archon_bin, env, log, item, trial_dir)
    state = (detail.get("item") or {}).get("state") or {}
    if status != "completed":
        result["workflow_error"] = state.get("error") or state.get("reason") or f"item {status}"
    ref = result_ref(workspace, detail)
    result["archon_result_ref"] = ref
    checkout(workspace, ref)


def run_archon_trial(archon_bin, aether_binary, condition, task_text, workspace, timeout, trial_dir, result,
                     extra_env, price):
    """Run one task through a private Archon on the candidate Aether; fills `result` like the other arms."""
    assert condition["kind"] == "archon" and condition["budget_usd"] > 0, "an Archon arm carries its budget"
    layout = archon_layout(trial_dir)
    prepare(layout, aether_binary, condition["model"], condition["max_concurrent_turns"])
    env, stripped = archon_env(layout, extra_env)
    log = []
    result["env_isolation"] = {"stripped_vars": stripped, "socket_in_runtime_dir": layout["socket_dir"] is not None}
    started = time.monotonic()
    try:
        _session(archon_bin, env, log, condition, task_text, workspace, timeout, trial_dir, result, layout)
    except (ArchonError, subprocess.TimeoutExpired, OSError) as error:
        result["workflow_status"] = "failed"
        result["workflow_error"] = f"{type(error).__name__}: {error}"
    finally:
        result["solve_seconds"] = round(time.monotonic() - started, 3)
        result["cleanup_error"] = _stop(archon_bin, env, log, layout)
        Path(trial_dir, "archon-commands.json").write_text(json.dumps(log, indent=2), encoding="utf-8")
    usage = store_usage(layout["aether_data"], price)
    result["archon_usage"] = usage
    result["usage_report"] = {"usage": {"output_tokens": usage.get("output_tokens")}}
    result["descendant_count"] = max(usage["agents"] - 2, 0)


def _stop(archon_bin, env, log, layout):
    """Stop archond and its aetherd and remove the private socket directory; returns any error text."""
    errors = []
    try:
        archon(archon_bin, env, ["daemon", "stop", "--json"], log)
    except (ArchonError, subprocess.TimeoutExpired, OSError) as error:
        errors.append(f"stopping archon: {error}")
    errors.append(remove_socket_dir(layout["socket_dir"]))
    return "; ".join(error for error in errors if error) or None
