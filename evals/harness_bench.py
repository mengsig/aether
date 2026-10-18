#!/usr/bin/env python3
"""Reproducible, paid benchmark runner comparing native Claude/Codex CLIs
against Aether single-agent and coordinator workflows on the same tasks.

See evals/harness_bench_protocol.md for the experiment design this
implements. This module never makes a paid model call itself: every
condition below drives a real `claude`/`codex` process or a real isolated
`aether` daemon as a subprocess, and only inspects their output.

Conditions (see CONDITIONS below for the exact prompts/models):
  native-claude               installed `claude` CLI, its own default model/effort.
  native-codex                installed `codex` CLI, its own default model/effort.
  aether-single-claude        one Aether agent, model claude-code/default.
  aether-single-codex         one Aether agent, model codex/default.
  aether-coordinator-team     Aether coordinator forced to staff exactly two
                               children (model claude-code/sonnet, high effort).
  aether-coordinator-default  Aether coordinator created idle (no objective),
                               then sent the task; free choice of delegation.

Usage:
  evals/.venv/bin/python evals/harness_bench.py \\
      --conditions native-claude,aether-single-claude --cases ledger --runs 1 \\
      --timeout 1800 --binary target/release/aether

Every trial's raw stdout/stderr, durable Aether events and usage accounting
land under target/hb/<short-id> (never /tmp: see evals/aether_client.py and
the repository's own scratch-space convention); a consolidated manifest with
one row per trial is written under --output-dir (default evals/results).
Trials run sequentially by default. `--jobs N` runs blocks of up to N trials
at once, keeping a case's matched native/Aether pair in one block: wall time
is then a paired comparison under shared load, not an absolute figure.
"""

import argparse
from contextlib import closing
import json
import os
import random
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time
import tempfile
import traceback
import uuid
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from aether_client import AetherClient, isolated_env, protocol
import harness_bench_cases as cases_module
import harness_bench_usage as usage_module
from harness_bench_reprice import rebuild_trial_usage
from harness_bench_treatments import binary_patches, is_diagnostic, loads_user_mcp, native_command, native_prompt
from harness_bench_metadata import (
    aether_resolved_metadata, native_configured_defaults, native_resolved_metadata,
)
from harness_bench_receipts import (
    _CLAUDE_PROXY, attribute_claude_receipts, install_claude_receipt_proxy, read_claude_receipts,
)

from harness_bench_config import (
    TEAM_CHILD_MODEL, TEAM_CHILD_EFFORT, COORDINATOR_EFFORT,
    COORDINATOR_TEAM_MODEL, COORDINATOR_DEFAULT_MODEL, COMMON_SUFFIX, SOLO_SUFFIX,
    TEAM_SUFFIX, DEFAULT_COORDINATOR_SUFFIX, CONDITIONS, DEFAULT_CONDITIONS,
    ALL_CONDITIONS, _AETHER_ISOLATED_ENV_KEYS, strip_live_routing_env,
    CLAUDE_PERMISSION_OVERRIDES, CODEX_PERMISSION_OVERRIDES, PROTOCOL_VERSION,
    native_claude_command, native_codex_command, sanitized_native_env, scheduler_config_toml,
)
from harness_bench_process import (
    run_subprocess_capturing, _terminate_process_group, own_process_group, disown_process_group,
)
from harness_bench_blocks import BLOCK_SCHEME, MAX_JOBS, host_load_average, paired_blocks, run_block
from harness_bench_wait import (
    TERMINAL_STATES, wait_for_run_terminal, wait_for_turn_reply, fetch_tree,
    tree_is_quiescent, wait_for_tree_quiescent, collect_run_events, create_agent_named,
)
from harness_bench_assets import (
    snapshot_frozen_files, frozen_violations, prepare_workspace, run_grade_with_tests,
    sha256_file, pin_binary, native_cli_provenance, benchmark_hashes,
    verify_benchmark_hashes, git_provenance,
)
from harness_bench_outcomes import (
    base_trial_record, _overall_success, _trial_status, protocol_violations,
    normalized_workflow_status, is_blocked_trial,
)

REPO_ROOT = os.path.dirname(HERE)
SCRATCH_ROOT = os.path.join(REPO_ROOT, "target", "hb")
SOCKET_PATH_LIMIT = 100  # sockaddr_un.sun_path is 108 bytes incl. the NUL; keep margin.

# ----- one trial --------------------------------------------------------


def _assert_socket_path_ok(path):
    assert len(path.encode()) < SOCKET_PATH_LIMIT, f"AF_UNIX socket path too long ({len(path.encode())}B): {path}"


def isolated_socket_path(trial_dir):
    """The trial's own socket path and, when one had to be made for it, its private directory.

    A deep checkout pushes `<trial_dir>/aetherd.sock` past the AF_UNIX limit; the
    socket then lives alone in a fresh 0700 directory under XDG_RUNTIME_DIR."""
    path = os.path.join(trial_dir, "aetherd.sock")
    if len(path.encode()) < SOCKET_PATH_LIMIT:
        return path, None
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime or not Path(runtime).is_dir():
        raise ValueError(f"socket path is too long and XDG_RUNTIME_DIR is unavailable: {path}")
    owned = tempfile.mkdtemp(prefix="hb-", dir=runtime)
    path = os.path.join(owned, "aetherd.sock")
    _assert_socket_path_ok(path)
    assert not os.listdir(owned), "a trial's socket directory must start empty"
    return path, owned


def remove_socket_dir(socket_dir):
    """Remove a private socket directory; returns the error text instead of hiding it."""
    if socket_dir is None:
        return None
    try:
        shutil.rmtree(socket_dir)
    except OSError as error:
        return f"{type(error).__name__}: {error}"
    return None


def save_json(path, obj):
    """Atomic write: a crash mid-matrix leaves the previous, not a torn, file."""
    tmp = f"{path}.tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=2, default=str)
    os.replace(tmp, path)


def run_native_trial(condition, task_text, workspace, timeout, trial_dir, result, extra_env=None):
    is_claude = condition["kind"] == "native-claude"
    command_builder = native_claude_command if is_claude else native_codex_command
    prompt = native_prompt(condition, task_text, workspace)
    assert task_text.strip() in prompt, "a native prompt always carries the whole task"
    command = native_command(condition, command_builder(prompt, condition.get("model"), condition.get("effort")))
    env, stripped_env = sanitized_native_env()
    native_env = condition.get("native_env") or {}
    assert not any(key.startswith("AETHER_") for key in native_env), "a native setting never routes to Aether"
    env.update(extra_env or {})
    env.update(native_env)
    result["env_isolation"] = {"stripped_vars": stripped_env, "extra_vars": sorted({*(extra_env or {}), *native_env})}
    result["configured_defaults"] = native_configured_defaults("claude" if is_claude else "codex")
    overrides = CLAUDE_PERMISSION_OVERRIDES if is_claude else CODEX_PERMISSION_OVERRIDES
    result["permission_overrides"] = [flag for flag in overrides if flag in command]
    stdout_path, stderr_path = os.path.join(trial_dir, "native-stdout.jsonl"), os.path.join(trial_dir, "native-stderr.log")
    run_info = run_subprocess_capturing(command, str(workspace), timeout, stdout_path, stderr_path, env=env)
    result.update(
        daemon_setup_seconds=0.0, solve_seconds=run_info["seconds"], command=command,
        exit_code=run_info["exit_code"], timed_out=run_info["timed_out"], cleanup_error=run_info["cleanup_error"],
        raw_logs={"stdout": stdout_path, "stderr": stderr_path},
        workflow_status="timeout" if run_info["timed_out"] else ("completed" if run_info["exit_code"] == 0 else "failed"),
    )
    with open(stdout_path, encoding="utf-8", errors="replace") as f:
        stdout_text = f.read()
    result["resolved_metadata"] = native_resolved_metadata(stdout_text, env, is_claude)
    parser = usage_module.parse_claude_jsonl if condition["kind"] == "native-claude" else usage_module.parse_codex_jsonl
    try:
        result["usage_report"] = parser(stdout_text)
    except usage_module.UsageError as error:
        result["usage_error"] = str(error)


def _spawn_root(client, condition, task_text):
    kind = condition["kind"]
    if kind == "aether-single":
        options = {"effort": condition["effort"]} if "effort" in condition else {}
        if "tools" in condition:
            options["tools"] = condition["tools"]
        if "budget" in condition:
            options["budget"] = condition["budget"]
        agent = client.create_agent(role="assistant", objective=task_text, model=condition["model"], long_lived=False, **options)
        return agent["id"], condition.get("effort")
    if kind == "aether-coordinator-team":
        agent = client.create_agent(role="coordinator", objective=task_text, model=condition["model"],
                                     effort=condition["effort"], long_lived=False)
        return agent["id"], condition["effort"]
    assert kind == "aether-coordinator-default", f"unhandled Aether condition kind: {kind!r}"
    agent = create_agent_named(client, role="coordinator", name="coordinator", model=condition["model"],
                                effort=condition["effort"], long_lived=True)
    client.send(agent["id"], task_text)
    return agent["id"], condition["effort"]


def _wait_root(client, condition, root_id, timeout):
    if condition["kind"] == "aether-coordinator-default":
        return wait_for_turn_reply(client, root_id, timeout)
    return wait_for_run_terminal(client, root_id, timeout)


def _record_tree(result, tree, root_id):
    nodes = tree.get("nodes", {})
    assert root_id in nodes, "tree missing its own root after a wait completed"
    mix_model = {"provider": "claude-code", "model": "sonnet"}
    children = []
    for child_id in nodes[root_id]["children"]:
        agent = nodes[child_id]["agent"]
        children.append({
            "id": agent["id"], "model": agent["model"], "role": agent["role"],
            "effort": agent.get("config", {}).get("effort"), "state": agent["status"]["state"],
            "staffed_as_mix": agent["model"] == mix_model and agent.get("config", {}).get("effort") == TEAM_CHILD_EFFORT,
        })
    result["direct_children"] = children
    result["descendant_count"] = len(nodes) - 1
    result["node_states"] = {agent_id: node["agent"]["status"]["state"] for agent_id, node in nodes.items()}


def _cancel_best_effort(client, root_id, timeout=5):
    try:
        client.request(protocol.cancel(agent=root_id, reason="harness_bench trial ended"), timeout=timeout)
        return None
    except Exception as error:  # noqa: BLE001 - shutdown continues unconditionally either way
        return str(error)


def start_daemon(binary, env, workspace):
    """Own the foreground process so partial startup cannot detach a leak."""
    log_path = Path(env["AETHER_DATA_DIR"]).parent / "daemon-stderr.log"
    with log_path.open("wb") as log:
        proc = subprocess.Popen([binary, "daemon", "--foreground"], env=env, cwd=workspace,
                                stdout=log, stderr=log, start_new_session=True)
    try:
        own_process_group(proc)
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"daemon exited {proc.returncode}; see {log_path}")
            if os.path.exists(env["AETHER_SOCKET"]):
                return proc
            time.sleep(0.1)
        raise TimeoutError(f"daemon did not listen within 60s; see {log_path}")
    except BaseException:
        _terminate_process_group(proc)
        disown_process_group(proc)
        raise


def stop_daemon(binary, env, proc):
    error = None
    try:
        if os.path.exists(env["AETHER_SOCKET"]):
            stopped = subprocess.run([binary, "stop"], env=env, capture_output=True,
                                     text=True, timeout=10, check=False)
            if stopped.returncode:
                error = f"daemon stop exited {stopped.returncode}: {stopped.stderr}"
        if proc is not None:
            proc.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired) as exc:
        error = str(exc)
    finally:
        if proc is not None:
            group_error = _terminate_process_group(proc)
            disown_process_group(proc)
            error = error or group_error
    if os.path.exists(env["AETHER_SOCKET"]):
        error = error or "daemon socket remains after shutdown"
    return error


def snapshot_metadata(snapshot):
    return {"id": snapshot.get("id"), "agent_id": snapshot.get("agent_id"),
            "model": snapshot.get("model"), "effort": snapshot.get("params", {}).get("effort"),
            "turn": snapshot.get("turn"), "adapter": snapshot.get("adapter")}


def capture_stopped_run(env, run_id, root_id, trial_dir, result, requested_effort, condition=None):
    """Read the durable store after shutdown, including cancellation tail events.

    Snapshot prompts/tools/messages never leave the database; metadata is allowlisted.
    """
    database = Path(env["AETHER_DATA_DIR"]) / "aether.db"
    if not database.exists():
        raise RuntimeError("no durable database available after daemon shutdown")
    with closing(sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True, timeout=5)) as db:
        if run_id is None:
            roots = db.execute("SELECT id, run FROM agents WHERE parent IS NULL").fetchall()
            if len(roots) > 1:
                raise RuntimeError("isolated trial has ambiguous root runs")
            if roots:
                root_id, run_id = roots[0]
                result["root_agent_id"] = root_id
        events = [json.loads(row[0]) for row in db.execute(
            "SELECT body FROM events WHERE run = ? ORDER BY seq", (run_id,))]
        agents = [json.loads(row[0]) for row in db.execute(
            "SELECT body FROM agents WHERE run = ?", (run_id,))]
        snapshots = [snapshot_metadata(json.loads(row[0])) for row in db.execute(
            "SELECT snapshots.body FROM snapshots JOIN agents ON agents.id = snapshots.agent_id "
            "WHERE agents.run = ? ORDER BY snapshots.created_at", (run_id,))]
    nodes = {a["id"]: {"agent": a, "children": []} for a in agents}
    for agent in agents:
        if agent.get("parent") in nodes:
            nodes[agent["parent"]]["children"].append(agent["id"])
    tree = {"nodes": nodes, "roots": [a["id"] for a in agents if a.get("parent") not in nodes]}
    if condition is not None and root_id in nodes:
        result["protocol_violations"] = sorted(set(result.get("protocol_violations", []))
                                               | set(protocol_violations(condition, tree, root_id)))
    raw = result.setdefault("raw_logs", {})
    for name, value in (("events", events), ("tree", tree), ("snapshots", snapshots)):
        raw[name] = str(Path(trial_dir) / f"{name}.json")
        save_json(raw[name], value)
    result["snapshot_metadata"] = snapshots
    try:
        result["resolved_metadata"] = aether_resolved_metadata(events, agents, env)
    except OSError as error:
        result["resolved_metadata_error"] = f"{type(error).__name__}: {error}"
    result["event_count"] = len(events)
    event_types = {}
    for event in events:
        kind = event.get("kind", {})
        event_type = kind.get("type") if isinstance(kind, dict) else None
        if isinstance(event_type, str):
            event_types[event_type] = event_types.get(event_type, 0) + 1
    result["event_type_counts"] = dict(sorted(event_types.items()))
    result["telemetry_capture"] = "durable store after daemon shutdown"
    result["tree_source"] = "durable agents reconstructed after shutdown; pre-cleanup live tree saved separately"
    if root_id in nodes:
        _record_tree(result, {"nodes": nodes}, root_id)
        result["blocked"] = is_blocked_trial(events, result, root_id)
    if root_id is None:
        result["usage_error"] = "no root agent was created"
        return
    try:
        result["usage_report"] = usage_module.parse_aether_events(
            events, root_id, initial_efforts={root_id: requested_effort})
    except usage_module.UsageError as error:
        result["usage_error"] = str(error)


def run_aether_trial(binary, condition, task_text, workspace, timeout, trial_dir, real_claude, result,
                     extra_env=None):
    """Run one trial on its own daemon; its private socket directory never outlives it."""
    env = isolated_env(trial_dir)
    result["env_isolation"] = {"stripped_vars": strip_live_routing_env(env, keep=_AETHER_ISOLATED_ENV_KEYS)}
    assert not set(extra_env or {}) & _AETHER_ISOLATED_ENV_KEYS, "extra_env never redirects the trial's daemon"
    env.update(extra_env or {})
    env["AETHER_SOCKET"], socket_dir = isolated_socket_path(trial_dir)
    try:
        result["env_isolation"]["socket_in_runtime_dir"] = socket_dir is not None
        _assert_socket_path_ok(env["AETHER_SOCKET"])
        _run_isolated_aether_trial(binary, env, condition, task_text, workspace, timeout, trial_dir,
                                   real_claude, result)
    finally:
        # The daemon is stopped by now; remove the directory even when stopping reported an error.
        errors = (result.get("daemon_cleanup_error"), remove_socket_dir(socket_dir))
        result["daemon_cleanup_error"] = "; ".join(error for error in errors if error) or None


def _run_isolated_aether_trial(binary, env, condition, task_text, workspace, timeout, trial_dir, real_claude, result):
    # Both sides of a Claude pair start without the machine user's MCP servers (protocol version 2).
    receipts_dir = install_claude_receipt_proxy(env, trial_dir, real_claude, strict_mcp=not loads_user_mcp(condition))
    if "max_concurrent_turns" in condition:
        config_path = Path(env["AETHER_CONFIG_DIR"], "config.toml")
        assert not config_path.exists(), "an isolated trial's config starts empty"
        config_path.write_text(scheduler_config_toml(condition["max_concurrent_turns"]), encoding="utf-8")
    setup_started = time.monotonic()
    client = proc = root_id = run_id = requested_effort = None
    solve_started = None
    try:
        proc = start_daemon(binary, env, str(workspace))
        result["daemon_setup_seconds"] = round(time.monotonic() - setup_started, 3)
        client = AetherClient(env["AETHER_SOCKET"])
        client.subscribe()
        solve_started = time.monotonic()
        root_id, requested_effort = _spawn_root(client, condition, task_text)
        result["root_agent_id"] = root_id
        run_id = client.get_agent(root_id)["run"]
        status, error, reply = _wait_root(client, condition, root_id, timeout)
        remaining = max(0.01, timeout - (time.monotonic() - solve_started))
        tree, quiescent = wait_for_tree_quiescent(client, run_id, root_id, remaining)
        result.update(workflow_status=normalized_workflow_status(condition, status, error, reply, tree, root_id),
                      workflow_error=error, tree_quiescent=quiescent)
        result["protocol_violations"] = protocol_violations(condition, tree, root_id)
        _record_tree(result, tree, root_id)
        tree_path = str(Path(trial_dir) / "tree-before-cleanup.json")
        result.setdefault("raw_logs", {})["tree_before_cleanup"] = tree_path
        save_json(tree_path, tree)
    except Exception as error:
        result.update(workflow_status="failed", workflow_error=f"{type(error).__name__}: {error}")
    finally:
        if solve_started is not None:
            result["solve_seconds"] = round(time.monotonic() - solve_started, 3)
        if client is not None and root_id is not None:
            # Cancel all active descendants even if the root already completed.
            try:
                tree = fetch_tree(client, run_id) if run_id else None
                active = [aid for aid, n in tree["nodes"].items()
                          if n["agent"]["status"]["state"] not in TERMINAL_STATES] if tree else [root_id]
                deadline = time.monotonic() + 10
                errors = []
                for aid in active:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        errors.append("cancellation deadline exhausted; forcing daemon shutdown")
                        break
                    errors.append(_cancel_best_effort(client, aid, min(5, remaining)))
                result["cancel_error"] = "; ".join(e for e in errors if e) or None
                if run_id:
                    _, settled = wait_for_tree_quiescent(client, run_id, root_id, 10)
                    result["cleanup_settled"] = settled
            except Exception as error:
                result["cancel_error"] = f"{type(error).__name__}: {error}"
        result.setdefault("daemon_setup_seconds", round(time.monotonic() - setup_started, 3))
        if client is not None:
            try:
                client.close()
            except OSError as error:
                result["client_close_error"] = str(error)
        result["daemon_cleanup_error"] = stop_daemon(binary, env, proc)
        if run_id is not None or (Path(env["AETHER_DATA_DIR"]) / "aether.db").exists():
            try:
                capture_stopped_run(env, run_id, root_id, trial_dir, result, requested_effort, condition)
            except Exception as error:
                result["telemetry_error"] = f"{type(error).__name__}: {error}"
        captures = attribute_claude_receipts(condition, root_id, read_claude_receipts(receipts_dir))
        result["claude_receipts"] = captures
        if captures and "usage_report" in result:
            try:
                result["claude_reconciliation"] = usage_module.reconcile_claude_receipts(result["usage_report"], captures)
            except usage_module.UsageError as error:
                result["claude_reconciliation_error"] = str(error)


def run_trial(binary, condition_key, case, run_index, timeout, real_claude, scratch_root, expected_hashes=None):
    condition = CONDITIONS[condition_key]
    short_id = uuid.uuid4().hex[:10]
    trial_dir = os.path.join(scratch_root, short_id)
    os.makedirs(trial_dir, exist_ok=False)
    result = base_trial_record(condition_key, case, run_index, short_id, trial_dir)
    result["requested_treatment"] = dict(condition)
    result_path = os.path.join(trial_dir, "result.json")
    save_json(result_path, result)
    before_digest = None
    expected_hashes = benchmark_hashes() if expected_hashes is None else expected_hashes
    result["input_hashes"] = expected_hashes
    try:
        result["integrity_violations"] = verify_benchmark_hashes(expected_hashes)
        if result["integrity_violations"]:
            raise RuntimeError("benchmark inputs changed before trial")
        workspace = Path(trial_dir) / "workspace"
        before_digest = prepare_workspace(case["id"], workspace)
        result["fixture_hashes"] = {str(p.relative_to(workspace)): sha256_file(p)
                                    for p in workspace.rglob("*") if p.is_file() and ".git" not in p.parts}
        task_text = case["objective"] + condition["suffix"]
        result.update(task_text=task_text, status="running")
        save_json(result_path, result)
        if condition["kind"] in ("native-claude", "native-codex"):
            run_native_trial(condition, task_text, workspace, timeout, trial_dir, result)
        else:
            run_aether_trial(binary, condition, task_text, workspace, timeout, trial_dir, real_claude, result)
    except Exception as error:  # noqa: BLE001 - one trial's crash must not lose the whole matrix
        result["harness_error"] = f"{type(error).__name__}: {error}"
        result["harness_traceback"] = traceback.format_exc()
        result["success"] = False
    finally:
        if before_digest is not None:
            result["contract_violations"] = frozen_violations(before_digest, snapshot_frozen_files(workspace))
            (result["grade"], result["grade_by_test"], result["grade_error"],
             result["grade_by_test_error"]) = run_grade_with_tests(case["id"], workspace)
        result["integrity_violations"] = sorted(set(result.get("integrity_violations", []))
                                                 | set(verify_benchmark_hashes(expected_hashes)))
        result["success"] = _overall_success(result)
        result["status"] = _trial_status(result)
        save_json(result_path, result)
    return result


# ----- CLI -------------------------------------------------------------


def parse_args(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--conditions", default=",".join(DEFAULT_CONDITIONS),
                     help=f"comma-separated condition ids; choose from {ALL_CONDITIONS}")
    ap.add_argument("--cases", default=None, help="comma-separated case ids (default: the first two)")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--seed", type=int, default=0, help="reproducible starting order for paired trials")
    ap.add_argument("--timeout", type=float, default=1800.0, help="per-trial solve timeout, seconds")
    ap.add_argument("--output-dir", default=os.path.join(HERE, "results"))
    ap.add_argument("--binary", default=os.path.join(HERE, "..", "target", "release", "aether"))
    ap.add_argument("--jobs", type=int, default=1,
                    help=f"trials run at once, 1 to {MAX_JOBS}; matched pairs always share a block (default 1)")
    ap.add_argument("--pilot", action="store_true",
                    help="mark the manifest as calibration; both reports refuse to count it")
    ap.add_argument("--diagnostic", action="store_true",
                    help="mark the manifest as an overhead diagnosis; required for diag- conditions, never counted")
    args = ap.parse_args(argv)
    if not args.diagnostic and any(is_diagnostic(key) for key in _resolve_conditions(args.conditions)):
        ap.error("diag- conditions run only with --diagnostic")
    if len(binary_patches([CONDITIONS[key] for key in _resolve_conditions(args.conditions)])) > 1:
        ap.error("an invocation has one --binary: an arm on a patched binary runs apart from other Aether arms")
    assert args.runs > 0, "--runs must be positive"
    assert args.timeout > 0, "--timeout must be positive"
    if not 1 <= args.jobs <= MAX_JOBS:
        ap.error(f"--jobs must be between 1 and {MAX_JOBS}")
    return args


def _resolve_conditions(spec):
    conditions = [c.strip() for c in spec.split(",") if c.strip()]
    unknown = sorted(set(conditions) - set(ALL_CONDITIONS))
    if unknown:
        raise SystemExit(f"unknown condition(s): {unknown}; choose from {ALL_CONDITIONS}")
    return conditions


def _resolve_cases(spec, all_cases):
    if not spec:
        return list(all_cases)[:2]
    case_ids = [c.strip() for c in spec.split(",") if c.strip()]
    unknown = sorted(set(case_ids) - set(all_cases))
    if unknown:
        raise SystemExit(f"unknown case id(s): {unknown}; choose from {sorted(all_cases)}")
    return case_ids


TRIAL_ORDERING = "seeded run-major latin rotation"


def trial_order(case_ids, conditions, runs, seed=0):
    """Counterbalanced trial sequence: runs are outer so every cell gets one
    observation before any gets a second, and within a run the condition order
    rotates per case (and per run) so no condition always runs first or last."""
    assert case_ids and conditions, "trial_order needs at least one case and one condition"
    assert runs > 0, "runs must be positive"
    assert len(set(case_ids)) == len(case_ids) and len(set(conditions)) == len(conditions), "cells must be unique"
    case_ids, conditions = list(case_ids), list(conditions)
    if seed:
        rng = random.Random(seed)
        rng.shuffle(case_ids)
        rng.shuffle(conditions)
    ordered = []
    for run_index in range(1, runs + 1):
        shift = run_index - 1
        rotated_cases = case_ids[shift % len(case_ids):] + case_ids[:shift % len(case_ids)]
        for position, case_id in enumerate(rotated_cases):
            offset = (shift + position) % len(conditions)
            for condition_key in conditions[offset:] + conditions[:offset]:
                ordered.append((case_id, condition_key, run_index))
    assert len(ordered) == len(case_ids) * len(conditions) * runs, "every cell must be scheduled exactly once per run"
    return ordered


def fetch_price_table(binary, binary_sha256, scratch_root):
    """Query only an owned daemon; no inherited live routing or model calls."""
    from harness_bench_pricing import validate_price_table

    if sha256_file(binary) != binary_sha256:
        raise ValueError("price-table binary does not match pinned SHA-256")
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime or not Path(runtime).is_dir():
        raise ValueError("price-table capture requires an existing XDG_RUNTIME_DIR")
    Path(scratch_root).mkdir(parents=True, exist_ok=True)
    # Config/data and stderr are retained for diagnosis unless capture succeeds.
    root = tempfile.mkdtemp(prefix="prices-", dir=Path(scratch_root).resolve())
    with tempfile.TemporaryDirectory(prefix="hbp-", dir=runtime) as sockets:
        env = isolated_env(root)
        strip_live_routing_env(env, keep=_AETHER_ISOLATED_ENV_KEYS)
        env["AETHER_SOCKET"] = str(Path(sockets) / "aetherd.sock")
        _assert_socket_path_ok(env["AETHER_SOCKET"])
        proc = start_daemon(binary, env, root)
        failure = None
        try:
            response = subprocess.run([binary, "models", "--json", "--socket", env["AETHER_SOCKET"]],
                                      env=env, cwd=root, capture_output=True, text=True,
                                      timeout=60, check=True)
            table = validate_price_table({"binary_sha256": binary_sha256,
                                          "source": "isolated daemon list_models",
                                          "models": json.loads(response.stdout)})
            if sha256_file(binary) != binary_sha256:
                raise ValueError("price-table binary changed during capture")
        except BaseException as error:
            failure = error
            raise
        finally:
            cleanup_error = stop_daemon(binary, env, proc)
            if cleanup_error and failure is not None:
                failure.add_note(f"price-table daemon cleanup: {cleanup_error}")
            elif cleanup_error:
                raise RuntimeError(f"price-table daemon cleanup: {cleanup_error}")
    try:
        shutil.rmtree(root)
    except OSError as error:
        raise RuntimeError(f"price-table scratch cleanup failed: {root}") from error
    return table


def build_manifest(argv, binary, binary_sha256, args):
    return {
        "command": "python3 evals/harness_bench.py " + " ".join(shlex.quote(x) for x in argv),
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "binary": {"pinned_path": binary, "sha256": binary_sha256, "source_argument": args.binary},
        "git": git_provenance(REPO_ROOT), "input_hashes": benchmark_hashes(),
        "native": {"claude": native_cli_provenance("claude"), "codex": native_cli_provenance("codex")},
        "conditions": _resolve_conditions(args.conditions), "runs": args.runs, "seed": args.seed,
        "timeout": args.timeout, "pilot": args.pilot, "diagnostic": args.diagnostic,
        "protocol_version": PROTOCOL_VERSION,
        "ordering": TRIAL_ORDERING, "jobs": args.jobs, "blocks": BLOCK_SCHEME,
    }


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    conditions = _resolve_conditions(args.conditions)
    all_cases = {c["id"]: c for c in cases_module.cases()}
    assert all_cases, "harness_bench_cases.cases() returned no cases"
    case_ids = _resolve_cases(args.cases, all_cases)

    output_dir = os.path.realpath(args.output_dir)
    os.makedirs(output_dir, exist_ok=True)
    os.makedirs(SCRATCH_ROOT, exist_ok=True)

    binary, binary_sha256 = pin_binary(os.path.realpath(args.binary), output_dir)
    real_claude = shutil.which("claude")
    manifest = build_manifest(sys.argv[1:] if argv is None else argv, binary, binary_sha256, args)
    manifest["price_table"] = fetch_price_table(binary, binary_sha256, SCRATCH_ROOT)
    manifest_path = os.path.join(output_dir, f"harness-bench-{time.strftime('%Y-%m-%dT%H%M%SZ', time.gmtime())}.json")
    rows = []
    save_json(manifest_path, {**manifest, "trials": rows})

    blocks = paired_blocks(trial_order(case_ids, conditions, args.runs, args.seed), args.jobs, args.seed)
    sequences = {trial: number for number, trial in enumerate((t for block in blocks for t in block), 1)}
    for block_index, block in enumerate(blocks, 1):
        def run_one(trial, position, block_index=block_index, block_trials=len(block)):
            placement = {"jobs": args.jobs, "block": block_index, "block_trials": block_trials,
                         "launch_position": position}
            return run_scheduled_trial(trial, sequences[trial], placement, all_cases, manifest,
                                       binary, args.timeout, real_claude)
        for row in run_block(block, run_one):
            rows.append(row)
            rows.sort(key=lambda r: r["sequence"])
            save_json(manifest_path, {**manifest, "trials": rows})
            print(f"   [{row['sequence']}] status={row.get('status')} success={row.get('success')} "
                  f"solve_seconds={row.get('solve_seconds')} "
                  f"error={row.get('harness_error') or row.get('workflow_error')}", flush=True)

    print(f"\n{len(rows)} trial(s) written to {manifest_path}")
    return manifest_path


def run_scheduled_trial(trial, sequence, placement, all_cases, manifest, binary, timeout, real_claude):
    """One trial of a block: its own scratch, plus where and under what load it ran."""
    case_id, condition_key, run_index = trial
    assert sequence > 0 and placement["launch_position"] <= placement["block_trials"], "impossible placement"
    print(f"-> [{sequence}] {condition_key} {case_id} run {run_index} "
          f"(block {placement['block']}, {placement['block_trials']} at once)", flush=True)
    load_start = host_load_average()
    row = run_trial(binary, condition_key, all_cases[case_id], run_index, timeout, real_claude,
                    SCRATCH_ROOT, manifest["input_hashes"])
    assert row["case"] == case_id and row["condition"] == condition_key, "trial row does not match its schedule"
    rebuild_trial_usage(row, manifest)
    row.update(sequence=sequence, concurrency=placement,
               host_load_average={"start": load_start, "end": host_load_average()})
    save_json(os.path.join(row["trial_dir"], "result.json"), row)
    return row


if __name__ == "__main__":
    main()
