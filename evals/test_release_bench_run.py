"""Release bench execution without a model: scheduling, the pool, resume, spend caps and the Archon launcher."""
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock
from collections import Counter
from contextlib import closing
from pathlib import Path

from release_bench_archon import archon_layout, run_archon_trial
from release_bench_arms import build_arm
from release_bench_pool import Gates, read_attempts, read_results, run_pool, schedule
from release_bench_spend import (
    ALLOWANCE_USD,
    MARKER,
    SpendGuard,
    max_call_usd,
    price_of,
    reservation_usd,
    stop_marked,
)
from release_bench_tasks import materialize
from release_bench_trial import (
    SHORT_TMP_MAX,
    _fresh_path,
    _guard,
    _price_from_store,
    _solve,
    trial_env,
    trial_name,
)

PRICE_TABLE = {"models": [
    {"model": {"provider": "claude-code", "model": "claude-haiku-5-5"},
     "capabilities": {"context_window": 200_000, "max_output_tokens": 64_000}},
    {"model": {"provider": "anthropic", "model": "claude-haiku-5-5"},
     "capabilities": {"cost": {"input_per_mtok": 1.0, "output_per_mtok": 5.0, "cache_read_per_mtok": 0.1,
                               "cache_write_per_mtok": 1.25}}}]}


def rec(task, arm, repeat, cost):
    model, harness = arm.split("/")
    return {"task": task, "arm": arm, "model": model, "harness": harness, "repeat": repeat, "passed": True,
            "fraction": 1.0, "cost_usd": cost, "infra_error": None, "harness_failure": None, "template": "t",
            "family": "f", "checks_passed": 1, "checks_total": 1, "seconds": 1}


class ScheduleTests(unittest.TestCase):
    def test_a_task_s_arms_stay_together_and_rotate(self):
        arms = ["haiku/native", "haiku/old", "haiku/new"]
        order = schedule(["a", "b", "c", "d"], arms, 2, seed=1)
        self.assertEqual(len(set(order)), 24)
        blocks = [order[index:index + 3] for index in range(0, 24, 3)]
        self.assertTrue(all(len({(task, repeat) for task, repeat, _ in block}) == 1 for block in blocks))
        self.assertGreater(len({block[0][2] for block in blocks}), 1)

    def test_old_runs_first_in_exactly_half_the_blocks_of_each_tier(self):
        arms = ["haiku/native", "haiku/old", "haiku/new", "luna/native", "luna/old", "luna/new"]
        for seed in range(5):
            order = schedule([f"t{index}" for index in range(10)], arms, 2, seed=seed)
            first = Counter()
            for start in range(0, len(order), len(arms)):
                block = [arm for _, _, arm in order[start:start + len(arms)]]
                for model in ("haiku", "luna"):
                    first[model] += block.index(f"{model}/old") < block.index(f"{model}/new")
            self.assertEqual(first, Counter({"haiku": 10, "luna": 10}), seed)


class PoolTests(unittest.TestCase):
    def _pool(self, directory, order, run_one, budget=10.0, reserve=0.7, jobs=3):
        gates = Gates(load_max=10_000, mem_min_gb=0, budget_usd=budget)
        records, stopped = run_pool(order, run_one, lambda _triple: reserve, jobs=jobs, gates=gates,
                                    results_path=str(Path(directory, "results.jsonl")),
                                    attempts_path=str(Path(directory, "attempts.jsonl")), log=lambda _line: None)
        return records, stopped, gates

    def test_the_pool_records_every_trial_and_resumes(self):
        order = schedule(["a", "b"], ["haiku/old", "haiku/new"], 2, seed=0)
        run_one = lambda triple: rec(triple[0], triple[2], triple[1], 0.01)  # noqa: E731
        with tempfile.TemporaryDirectory() as directory:
            records, stopped, _ = self._pool(directory, order[:5], run_one)
            self.assertIsNone(stopped)
            self.assertEqual(len(records), 5)
            records, _, gates = self._pool(directory, order, run_one)
            self.assertEqual(len(read_results(str(Path(directory, "results.jsonl")))), 8)
            self.assertAlmostEqual(gates.spent_usd, 0.08)
            self.assertEqual(len(read_attempts(str(Path(directory, "attempts.jsonl")))), 8)

    def test_runaway_trials_cannot_spend_past_the_budget(self):
        # Every trial spends its whole worst case: the allowance plus the overshoot its reservation bounds.
        worst = reservation_usd(PRICE_TABLE, "haiku")
        order = schedule([f"t{index}" for index in range(40)], ["haiku/old"], 1, seed=0)
        run_one = lambda triple: rec(triple[0], triple[2], triple[1], worst)  # noqa: E731
        with tempfile.TemporaryDirectory() as directory:
            records, stopped, gates = self._pool(directory, order, run_one, budget=5.0, reserve=worst, jobs=4)
        self.assertIn("budget", stopped)
        self.assertLessEqual(sum(item["cost_usd"] for item in records), 5.0)
        self.assertEqual(len(records), int(5.0 // worst))
        self.assertAlmostEqual(gates.reserved_usd, 0.0)

    def test_an_unpriced_trial_is_charged_its_whole_reservation(self):
        gates = Gates(load_max=1, mem_min_gb=0, budget_usd=1.0)
        self.assertTrue(gates.reserve(0.5))
        self.assertTrue(gates.reserve(0.4))
        self.assertFalse(gates.reserve(0.2))
        gates.settle(0.5, 0.05)
        gates.settle(0.4, None)
        self.assertAlmostEqual(gates.spent_usd, 0.45)
        self.assertAlmostEqual(gates.reserved_usd, 0.0)

    def test_a_resume_moves_an_interrupted_trial_aside_and_charges_it(self):
        with tempfile.TemporaryDirectory() as directory:
            trials = Path(directory, "trials")

            def interrupted(triple):
                (trials / trial_name(triple[0], triple[2], triple[1])).mkdir(parents=True)
                raise KeyboardInterrupt

            def fresh(triple):
                trial_dir = _fresh_path(trials / trial_name(triple[0], triple[2], triple[1]))
                trial_dir.mkdir(parents=True, exist_ok=False)
                return rec(triple[0], triple[2], triple[1], 0.01)

            order = [("a", 0, "haiku/old")]
            with self.assertRaises(KeyboardInterrupt):
                self._pool(directory, order, interrupted, jobs=1)
            records, _, gates = self._pool(directory, order, fresh, jobs=1)
            self.assertEqual(len(records), 1)
            self.assertAlmostEqual(gates.spent_usd, 0.7 + 0.01)
            self.assertTrue(Path(trials, "a--haiku-old--r0.abandoned-1").is_dir())
            self.assertTrue(Path(trials, "a--haiku-old--r0").is_dir())
            _, _, gates = self._pool(directory, order, fresh, jobs=1)
            self.assertAlmostEqual(gates.spent_usd, 0.7 + 0.01, msg="a recorded trial's own start is not abandoned")

    def test_a_torn_last_attempt_line_never_ran(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "attempts.jsonl")
            start = {"task": "a", "repeat": 0, "arm": "haiku/old", "reserve_usd": 0.7}
            path.write_text(json.dumps(start) + "\n" + '{"task": "b", "rep')
            self.assertEqual(read_attempts(str(path)), [start])


class SpendTests(unittest.TestCase):
    def test_the_reservation_covers_the_allowance_and_the_overshoot(self):
        self.assertAlmostEqual(max_call_usd(PRICE_TABLE, "haiku"), (200_000 * 1.25 + 64_000 * 5.0) / 1e6)
        one = reservation_usd(PRICE_TABLE, "haiku")
        self.assertAlmostEqual(one, ALLOWANCE_USD["haiku"] + 2 * 0.57)
        self.assertAlmostEqual(reservation_usd(PRICE_TABLE, "haiku", agents=3) - one, 4 * 0.57)
        with self.assertRaises(ValueError):
            price_of({"models": []}, "luna")

    def test_the_guard_stops_a_trial_at_its_allowance(self):
        readings, stopped = iter([0.1, 0.3, 0.6, 0.9]), []
        guard = SpendGuard(lambda: next(readings, 0.9), 0.5, "m", poll_seconds=0.01, stop=stopped.append)
        with guard:
            deadline = time.monotonic() + 5
            while not stopped and time.monotonic() < deadline:
                time.sleep(0.01)
        self.assertEqual(stopped, ["m"])
        self.assertAlmostEqual(guard.peak_usd, 0.6)
        self.assertIn("allowance", guard.tripped)

    def test_a_broken_probe_stops_the_trial(self):
        stopped = []

        def probe():
            raise KeyError("usage")
        with SpendGuard(probe, 0.5, "m", poll_seconds=0.01, stop=stopped.append) as guard:
            deadline = time.monotonic() + 5
            while not stopped and time.monotonic() < deadline:
                time.sleep(0.01)
        self.assertEqual(stopped, ["m"])
        self.assertIn("KeyError", guard.tripped)

    def test_stop_marked_ends_every_marked_process(self):
        value = f"test-{os.getpid()}-{time.monotonic_ns()}"
        child = subprocess.Popen(["sleep", "30"], env={**os.environ, MARKER: value})
        try:
            self.assertEqual(stop_marked(value), [child.pid])
            self.assertIsNotNone(child.wait(timeout=10))
        finally:
            child.kill()
            child.wait()



def write_store(data_dir, agents):
    """An Aether store holding one cumulative usage event per named agent."""
    data_dir.mkdir(parents=True)
    with closing(sqlite3.connect(data_dir / "aether.db")) as db, db:
        db.execute("CREATE TABLE events (seq INTEGER PRIMARY KEY, body TEXT)")
        for seq, agent in enumerate(agents, 1):
            total = {"input_tokens": 1000, "output_tokens": 100, "cache_read_tokens": 0, "cache_write_tokens": 0,
                     "reasoning_tokens": 0, "cost_usd": 0.0, "cost_complete": False}
            body = {"agent_id": agent, "kind": {"type": "agent.usage_updated", "total": total}}
            db.execute("INSERT INTO events VALUES (?, ?)", (seq, json.dumps(body)))


def unit(agent, cost, tokens):
    return {"agent_id": agent, "usage": {"api_equivalent_cost_usd": cost, "total_tokens": tokens}}


class TrialIsolationTests(unittest.TestCase):
    CONTEXT = {"price_manifest": {"price_table": PRICE_TABLE}}
    ARM = {"model": "haiku", "harness": "new"}

    def test_a_deep_trial_directory_still_gets_a_socket_sized_tmpdir(self):
        with tempfile.TemporaryDirectory() as root:
            trial_dir = Path(root, *["nested-directory-name"] * 6)
            env, other = trial_env(trial_dir, "m"), trial_env(Path(root, "other"), "n")
            try:
                socket = Path(env["TMPDIR"], "aether-codex-abcdef", "exec.sock")
                self.assertLessEqual(len(env["TMPDIR"]), SHORT_TMP_MAX)
                self.assertLess(len(str(socket).encode()), 108)
                self.assertFalse(Path(env["TMPDIR"]).is_relative_to(trial_dir))
                self.assertNotEqual(env["TMPDIR"], other["TMPDIR"])
            finally:
                os.rmdir(env["TMPDIR"])
                os.rmdir(other["TMPDIR"])

    def test_the_tmpdir_is_removed_when_the_harness_crashes(self):
        seen = {}

        def crash(*args):
            seen["tmp"] = Path(args[-1]["TMPDIR"])
            (seen["tmp"] / "exec.sock").write_text("x")
            raise RuntimeError("harness crashed")
        with tempfile.TemporaryDirectory() as root, unittest.mock.patch("release_bench_trial._solve_in", crash):
            result = {}
            with self.assertRaises(RuntimeError):
                _solve(self.ARM, "task", root, Path(root, "trial"), self.CONTEXT, result, "m")
        self.assertFalse(seen["tmp"].exists())
        self.assertNotIn("tmpdir_cleanup_error", result)

    def _row(self, root, units, store_agents):
        write_store(Path(root, "data"), store_agents)
        return {"trial_dir": root, "usage_report": {"units": units}}

    def test_turns_that_never_reached_a_model_do_not_unprice_the_trial(self):
        units = [unit("a", 0.0015, 1100), unit("a", None, 0), unit("b", None, None), unit("c", None, None)]
        with tempfile.TemporaryDirectory() as root:
            row = self._row(root, units, ["a"])
            self.assertAlmostEqual(_price_from_store(row, self.ARM, self.CONTEXT), 0.0015)
            self.assertIn("3 unit(s)", row["price_note"])

    def test_an_unpriced_unit_with_tokens_keeps_the_trial_unpriced(self):
        with tempfile.TemporaryDirectory() as root:
            row = self._row(root, [unit("a", 0.0015, 1100), unit("b", None, 1100)], ["a", "b"])
            self.assertIsNone(_price_from_store(row, self.ARM, self.CONTEXT))
            self.assertNotIn("price_note", row)

    def test_a_store_agent_missing_from_the_report_is_a_bug(self):
        with tempfile.TemporaryDirectory() as root:
            row = self._row(root, [unit("a", 0.0015, 1100), unit("b", None, None)], ["a", "z"])
            with self.assertRaises(AssertionError):
                _price_from_store(row, self.ARM, self.CONTEXT)

FAKE_ARCHON = r'''
import json, os, shutil, sqlite3, subprocess, sys
data = os.environ["ARCHON_DATA_DIR"]
state_path = os.path.join(data, "fake-state.json")
state = json.load(open(state_path)) if os.path.exists(state_path) else {"calls": []}
args = sys.argv[1:]
state["calls"].append(args)
state.setdefault("aether_env", sorted(key for key in os.environ if key.startswith("AETHER_")))
code, out = 0, {}
if args[0] == "setup":
    config = open(os.path.join(os.environ["ARCHON_CONFIG_DIR"], "config.toml")).read()
    assert 'mode = "dedicated"' in config and "budgets_enabled = true" in config
elif args[:2] == ["daemon", "run"]:
    state["aether"] = os.path.realpath(shutil.which("aether"))
elif args[:2] == ["repo", "add"]:
    state["repo"] = args[2]
elif args[:2] == ["queue", "add"]:
    state["budget"] = args[args.index("--budget-usd") + 1]
    out = {"item": {"id": "qi_fake"}}
elif args[:2] == ["queue", "wait"]:
    code = int(os.environ.get("FAKE_WAIT_CODE", "0"))
    tree = os.path.join(data, "worktree")
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@t"]
    subprocess.run(git + ["-C", state["repo"], "worktree", "add", "-q", "-b", "archon/fake", tree], check=True)
    open(os.path.join(tree, "fixed.txt"), "w").write("done\n")
    subprocess.run(git + ["-C", tree, "add", "-A"], check=True)
    subprocess.run(git + ["-C", tree, "commit", "-q", "-m", "fix"], check=True)
    os.makedirs(os.path.join(data, "aether"), exist_ok=True)
    db = sqlite3.connect(os.path.join(data, "aether", "aether.db"))
    db.execute("CREATE TABLE events (seq INTEGER PRIMARY KEY, body TEXT)")
    for seq, agent in enumerate(["orchestrator", "stage", "child"], 1):
        total = {"input_tokens": 1000, "output_tokens": 100, "cache_read_tokens": 0, "cache_write_tokens": 0,
                 "reasoning_tokens": 0, "cost_usd": 0.0015, "cost_complete": True}
        body = {"agent_id": agent, "kind": {"type": "agent.usage_updated", "total": total}}
        db.execute("INSERT INTO events VALUES (?, ?)", (seq, json.dumps(body)))
    db.commit()
elif args[:2] == ["queue", "show"]:
    out = {"item": {"id": "qi_fake", "state": {"state": "succeeded"}},
           "runs": [{"branch": "archon/fake", "started_at": "2026-10-18T00:00:00Z", "cost_usd": 0.0045}]}
json.dump(state, open(state_path, "w"))
print(json.dumps(out))
sys.exit(code)
'''


class ArchonLauncherTests(unittest.TestCase):
    SPEC = {"id": "tiny-0", "family": "f", "template": "tiny", "seed": 0, "prompt": "fix it",
            "files": {"a.py": "x = 1\n"}, "history": [], "tags": {}, "meta": {}, "hidden": "import unittest\n",
            "checks": 2, "solution": {"a.py": "x = 2\n"}}

    def _run(self, directory, wait_code="0"):
        root = Path(directory)
        fake = root / "archon"
        fake.write_text(f"#!{sys.executable}\n{FAKE_ARCHON}")
        fake.chmod(0o755)
        candidate = root / "candidate-aether"
        candidate.write_text("#!/bin/sh\nexit 0\n")
        candidate.chmod(0o755)
        workspace, trial_dir = root / "work" / "workspace", root / "trial"
        materialize(self.SPEC, workspace)
        trial_dir.mkdir()
        condition = build_arm("haiku", "archon")["condition"]
        result = {}
        run_archon_trial(fake, candidate, condition, "fix it", workspace, 30, str(trial_dir), result,
                         {"FAKE_WAIT_CODE": wait_code}, price_of(PRICE_TABLE, "haiku"))
        state = json.loads(Path(archon_layout(str(trial_dir))["data"], "fake-state.json").read_text())
        return result, state, workspace, candidate

    def test_a_task_runs_on_a_private_archon_bound_to_the_candidate(self):
        with tempfile.TemporaryDirectory() as directory:
            result, state, workspace, candidate = self._run(directory)
            self.assertEqual(result["workflow_status"], "completed")
            self.assertEqual(state["aether"], str(candidate.resolve()))
            self.assertEqual(state["aether_env"], ["AETHER_CONFIG_DIR"])
            self.assertEqual(state["budget"], f"{ALLOWANCE_USD['haiku']:.2f}")
            self.assertEqual((workspace / "fixed.txt").read_text(), "done\n")
            self.assertEqual(result["archon_result_ref"], "archon/fake")
            self.assertEqual(state["calls"][-1], ["daemon", "stop", "--json"])
            self.assertIsNone(result["cleanup_error"])
            usage = result["archon_usage"]
            self.assertEqual((usage["agents"], usage["total_tokens"], result["descendant_count"]), (3, 3300, 1))
            self.assertGreater(usage["api_equivalent_usd"], 0)

    def test_a_timed_out_item_is_cancelled(self):
        with tempfile.TemporaryDirectory() as directory:
            result, state, _, _ = self._run(directory, wait_code="4")
            self.assertEqual(result["workflow_status"], "timeout")
            self.assertIn(["queue", "cancel", "qi_fake", "--json"], state["calls"])
            self.assertEqual(state["calls"][-1], ["daemon", "stop", "--json"])

    def test_a_failing_archon_is_a_failed_workflow_not_a_crash(self):
        with tempfile.TemporaryDirectory() as directory:
            result, state, workspace, _ = self._run(directory, wait_code="1")
            self.assertEqual(result["workflow_status"], "failed")
            self.assertIn("ArchonError", result["workflow_error"])
            self.assertEqual(state["calls"][-1], ["daemon", "stop", "--json"])

    def test_the_guard_reads_the_archon_trial_s_own_store(self):
        with tempfile.TemporaryDirectory() as directory:
            context = {"price_manifest": {"price_table": PRICE_TABLE}}
            guard = _guard(build_arm("haiku", "archon"), directory, context, "m")
            self.assertEqual(guard.probe.database.parent.resolve(), archon_layout(directory)["aether_data"])
            self.assertEqual(guard.allowance_usd, ALLOWANCE_USD["haiku"])

if __name__ == "__main__":
    unittest.main()
