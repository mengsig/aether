"""Release bench execution without a model: scheduling, the pool, resume, spend caps and the Archon launcher."""
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import unittest.mock
from collections import Counter
from contextlib import closing
from pathlib import Path

import harness_bench as hb
import release_bench
import release_bench_pool
import release_bench_trial
from harness_bench_config import scheduler_config_toml
from release_bench_archon import archon_layout, run_archon_trial
from release_bench_arms import agent_cap, build_arm
from release_bench_pool import Gates, read_attempts, read_results, run_pool, schedule
from release_bench_spend import (
    ALLOWANCE_USD,
    MARKER,
    ClaudeStreamProbe,
    CodexRolloutProbe,
    ProbeError,
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
            path.write_text(json.dumps(start) + "\n" + "garbage\n")
            with self.assertRaises(ValueError):
                read_attempts(str(path))

    def test_two_resumes_after_a_torn_attempt_line(self):
        def run_one(triple):
            return rec(triple[0], triple[2], triple[1], 0.01)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "attempts.jsonl")
            path.write_text('{"task": "torn", "rep')
            self._pool(directory, [("a", 0, "haiku/old")], run_one, jobs=1)
            self.assertEqual([item["task"] for item in read_attempts(str(path))], ["a"])
            records, _, gates = self._pool(directory, [("a", 0, "haiku/old"), ("b", 0, "haiku/old")], run_one, jobs=1)
            self.assertEqual([item["task"] for item in read_attempts(str(path))], ["a", "b"])
            self.assertEqual(len(records), 2)
            self.assertAlmostEqual(gates.spent_usd, 0.02, msg="the torn start never ran, so it is not charged")


class HostRecheckTests(unittest.TestCase):
    def test_load_that_rises_during_a_budget_wait_holds_the_next_trial(self):
        load, readings, first_checks = {"value": 0.0}, [], threading.Event()
        started = {}

        def getloadavg():
            readings.append(load["value"])
            if len(readings) == 2:
                first_checks.set()
            if load["value"] > 16 and readings.count(load["value"]) >= 3:
                load["value"] = 0.0  # The load recovers after a few polls.
            return (readings[-1], 0.0, 0.0)

        def run_one(triple):
            started[triple[0]] = list(readings)
            if triple[0] == "a":
                # The second trial passed its first host check and now waits for budget; load spikes.
                self.assertTrue(first_checks.wait(5))
                load["value"] = 99.0
            return rec(triple[0], triple[2], triple[1], 0.01)
        gates = Gates(load_max=16, mem_min_gb=0, budget_usd=1.0)
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(release_bench_pool.os, "getloadavg", getloadavg), \
                unittest.mock.patch.object(release_bench_pool, "GATE_POLL_SECONDS", 0.01):
            records, stopped = run_pool([("a", 0, "haiku/old"), ("b", 0, "haiku/old")], run_one, lambda _: 0.7,
                                        jobs=2, gates=gates, results_path=str(Path(directory, "r.jsonl")),
                                        attempts_path=str(Path(directory, "a.jsonl")), log=lambda _line: None)
        self.assertIsNone(stopped)
        self.assertEqual(len(records), 2)
        self.assertIn(99.0, started["b"], "the gate looked at the host again after the budget wait")
        self.assertLessEqual(started["b"][-1], 16, "the second trial started only once the load recovered")


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

    def _guard_run(self, probe, seconds=0.3, unobservable=0.08):
        stopped = []
        with SpendGuard(probe, 0.5, "m", poll_seconds=0.01, stop=stopped.append,
                        unobservable_seconds=unobservable) as guard:
            deadline = time.monotonic() + seconds
            while not stopped and time.monotonic() < deadline:
                time.sleep(0.01)
        return guard, stopped

    def test_spend_that_stays_unknown_stops_the_trial(self):
        guard, stopped = self._guard_run(lambda: None)
        self.assertEqual(stopped, ["m"])
        self.assertTrue(guard.unobserved)
        self.assertIn("could not be observed", guard.tripped)

    def test_spend_known_again_within_the_grace_period_keeps_the_trial(self):
        started = time.monotonic()

        def probe():
            # Unknown, then unreadable, for less than the grace period; then a source appears.
            elapsed = time.monotonic() - started
            if elapsed < 0.03:
                return None
            if elapsed < 0.06:
                raise ProbeError("locked")
            return 0.1
        guard, stopped = self._guard_run(probe, unobservable=0.15)
        self.assertEqual(stopped, [])
        self.assertIsNone(guard.tripped)
        self.assertAlmostEqual(guard.peak_usd, 0.1)

    def test_probes_tell_a_missing_source_from_zero_spend(self):
        price = price_of(PRICE_TABLE, "haiku")
        with tempfile.TemporaryDirectory() as root:
            stdout, home = Path(root, "stdout.jsonl"), Path(root, "codex")
            self.assertIsNone(ClaudeStreamProbe(stdout, price)())
            stdout.write_text('{"type": "system"}\n')
            self.assertEqual(ClaudeStreamProbe(stdout, price)(), 0)
            codex = CodexRolloutProbe(stdout, price, codex_home=home)
            self.assertIsNone(codex(), "no thread yet")
            with stdout.open("a") as stream:
                stream.write('{"type": "thread.started", "thread_id": "abc"}\n')
            self.assertIsNone(codex(), "a thread whose rollout cannot be found")
            day = time.localtime()
            folder = home / "sessions" / f"{day.tm_year:04d}" / f"{day.tm_mon:02d}" / f"{day.tm_mday:02d}"
            folder.mkdir(parents=True)
            (folder / "rollout-2026-abc.jsonl").write_text('{"type": "session_meta"}\n')
            self.assertEqual(codex(), 0.0, "a rollout before its first finished call")

    def test_stop_marked_ends_every_marked_process(self):
        value = f"test-{os.getpid()}-{time.monotonic_ns()}"
        child = subprocess.Popen(["sleep", "30"], env={**os.environ, MARKER: value})
        try:
            self.assertEqual(stop_marked(value), [child.pid])
            self.assertIsNotNone(child.wait(timeout=10))
        finally:
            child.kill()
            child.wait()



def codex_rollout(folder, started, thread, meta, totals):
    """A Codex session rollout: its session_meta header, then one cumulative token count per (input, output)."""
    lines = [{"type": "session_meta", "payload": {"id": thread, **meta}}]
    for tokens_in, tokens_out in totals:
        usage = {"input_tokens": tokens_in, "cached_input_tokens": 0, "cache_write_input_tokens": 0,
                 "output_tokens": tokens_out, "reasoning_output_tokens": 0, "total_tokens": tokens_in + tokens_out}
        lines.append({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": usage}}})
    path = folder / f"rollout-{started}-{thread}.jsonl"
    path.write_text("".join(json.dumps(line) + "\n" for line in lines))
    return path


class CodexDescendantTests(unittest.TestCase):
    """A native Codex root that idles while its subagent spends: the guard and the final cost both see the child."""

    def _session(self, root):
        stdout, home = Path(root, "native-stdout.jsonl"), Path(root, "codex")
        stdout.write_text('{"type": "thread.started", "thread_id": "root"}\n')
        day = time.localtime()
        folder = home / "sessions" / f"{day.tm_year:04d}" / f"{day.tm_mon:02d}" / f"{day.tm_mday:02d}"
        folder.mkdir(parents=True)
        codex_rollout(folder, "2026-10-18T05-00-00", "earlier", {"session_id": "root"}, [(10**6, 10**5)])
        codex_rollout(folder, "2026-10-18T05-08-37", "root", {"session_id": "root"}, [(1000, 100)])
        return stdout, home, folder

    def test_a_child_crossing_the_allowance_stops_an_idle_root(self):
        price = price_of(PRICE_TABLE, "haiku")
        with tempfile.TemporaryDirectory() as root:
            stdout, home, folder = self._session(root)
            probe = CodexRolloutProbe(stdout, price, codex_home=home)
            self.assertAlmostEqual(probe(), 0.0015, msg="the root alone; an older rollout is never adopted")
            codex_rollout(folder, "2026-10-18T05-08-40", "stranger", {"session_id": "other"}, [(10**6, 10**5)])
            codex_rollout(folder, "2026-10-18T05-08-46", "child",
                                  {"session_id": "root", "parent_thread_id": "root"}, [(1000, 100), (10**6, 10**5)])
            codex_rollout(folder, "2026-10-18T05-08-50", "grandchild", {"parent_thread_id": "child"}, [(2000, 0)])
            stopped = []
            with SpendGuard(probe, 0.15, "m", poll_seconds=0.01, stop=stopped.append) as guard:
                deadline = time.monotonic() + 2
                while not stopped and time.monotonic() < deadline:
                    time.sleep(0.01)
            self.assertEqual(stopped, ["m"])
            self.assertAlmostEqual(guard.peak_usd, 0.0015 + 1.5 + 0.002)
            self.assertEqual(set(probe.totals), {"root", "child", "grandchild"})
            self.assertEqual(probe.descendants()["child"], (1.5, 1_100_000))

    def test_a_rollout_whose_header_is_still_being_written_is_read_later(self):
        price = price_of(PRICE_TABLE, "haiku")
        with tempfile.TemporaryDirectory() as root:
            stdout, home, folder = self._session(root)
            probe = CodexRolloutProbe(stdout, price, codex_home=home)
            torn = folder / "rollout-2026-10-18T05-08-46-child.jsonl"
            torn.write_text('{"type": "session_meta", "payload": {"id": "child", "session_')
            self.assertAlmostEqual(probe(), 0.0015)
            codex_rollout(folder, "2026-10-18T05-08-46", "child", {"session_id": "root"}, [(10**6, 0)])
            self.assertAlmostEqual(probe(), 1.0015)

    def test_settlement_charges_the_children_and_counts_them(self):
        price = price_of(PRICE_TABLE, "haiku")
        arm = {"model": "luna", "harness": "native"}
        summary = {"cost_priced": True, "api_equivalent_usd": 0.0015, "tokens": 1100}
        with tempfile.TemporaryDirectory() as root, \
                unittest.mock.patch.object(release_bench_trial, "rebuild_trial_usage", lambda *_: None), \
                unittest.mock.patch.object(release_bench_trial, "trial_summary", lambda _row: dict(summary)), \
                unittest.mock.patch.object(release_bench_trial, "price_of", lambda *_: price):
            stdout, home, folder = self._session(root)
            codex_rollout(folder, "2026-10-18T05-08-46", "child", {"session_id": "root"}, [(10**6, 10**5)])
            row = {"raw_logs": {"stdout": str(stdout)}}
            with unittest.mock.patch.dict(os.environ, {"CODEX_HOME": str(home)}):
                cost, tokens = release_bench_trial._price(row, arm, {"price_manifest": {"price_table": {}}})
                self.assertAlmostEqual(cost, 1.5015)
                self.assertEqual((tokens, row["descendant_count"]), (1100 + 1_100_000, 1))
                (folder / "rollout-2026-10-18T05-08-37-root.jsonl").unlink()
                cost, _ = release_bench_trial._price(row, arm, {"price_manifest": {"price_table": {}}})
        self.assertIsNone(cost, "without the root's rollout the subagents' spend is unknown")
        self.assertIn("subagents' spend is unknown", row["price_note"])


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

class AgentCapTests(unittest.TestCase):
    """Every arm installs the cap on agents at once that its budget reservation counts."""
    SOLO = {"id": "solo-0", "meta": {}}
    DELEGATION = {"id": "team-0", "meta": {"allow_delegation": True}}

    def test_the_caps_follow_each_harness_s_limit(self):
        expected = {("native", False): 2, ("native", True): 5, ("old", False): 1, ("new", True): 5,
                    ("archon", False): 2, ("archon", True): 6}
        self.assertEqual({key: agent_cap(*key) for key in expected}, expected)
        for model in ("haiku", "luna"):
            for harness in ("old", "new", "archon"):
                for delegation in (False, True):
                    arm = build_arm(model, harness, allow_delegation=delegation)
                    self.assertEqual(arm["condition"]["max_concurrent_turns"], arm["max_agents"])
        native = build_arm("haiku", "native", allow_delegation=True)
        self.assertEqual(native["condition"]["native_env"], {"CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS": "4"})
        luna = build_arm("luna", "native")["condition"]
        self.assertEqual(luna["codex_config"], ["-c", "agents.max_concurrent_threads_per_session=1",
                                                "-c", "features.multi_agent_v2.max_concurrent_threads_per_session=1"])
        with self.assertRaises(AssertionError):
            scheduler_config_toml(0)

    def test_the_reservation_counts_the_cap_the_trial_runs_under(self):
        seen = {}

        def pool(order, run_one, reservation_of, **_):
            seen["reserve"] = {triple: reservation_of(triple) for triple in order}
            seen["arms"] = {triple: run_one(triple) for triple in order}
            return [], None
        args = unittest.mock.Mock(repeats=1, seed=0, load_max=16, mem_min_gb=0, budget=100, jobs=1)
        keys = ["haiku/native", "haiku/old", "haiku/archon"]
        context = {"price_manifest": {"price_table": PRICE_TABLE}}
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(release_bench, "run_pool", pool), \
                unittest.mock.patch.object(release_bench, "run_release_trial", lambda spec, arm, *_: arm):
            release_bench.execute([self.SOLO, self.DELEGATION], keys, args, Path(directory), context)
        self.assertEqual(len(seen["reserve"]), 6)
        for triple, arm in seen["arms"].items():
            self.assertEqual(seen["reserve"][triple], reservation_usd(PRICE_TABLE, "haiku", arm["max_agents"]))
        caps = {(task, key): arm["max_agents"] for (task, _, key), arm in seen["arms"].items()}
        self.assertEqual(caps[("team-0", "haiku/old")], 5)
        self.assertEqual(caps[("solo-0", "haiku/archon")], 2)

    def test_an_aether_trial_s_daemon_starts_under_the_cap(self):
        condition = build_arm("haiku", "new", allow_delegation=True)["condition"]
        seen = {}

        def start(binary, env, workspace):
            seen["config"] = Path(env["AETHER_CONFIG_DIR"], "config.toml").read_text()
            raise RuntimeError("no daemon in a unit test")
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(hb, "install_claude_receipt_proxy", return_value=directory), \
                unittest.mock.patch.object(hb, "read_claude_receipts", return_value=[]), \
                unittest.mock.patch.object(hb, "stop_daemon", return_value=None), \
                unittest.mock.patch.object(hb, "start_daemon", start):
            result = {}
            hb._run_isolated_aether_trial("aether", hb.isolated_env(directory), condition, "task", directory, 5,
                                          directory, "claude", result)
        self.assertEqual(seen["config"], "[scheduler]\nmax_concurrent_turns = 5\n")
        self.assertIn("no daemon", result["workflow_error"])

    def test_native_trials_start_under_the_subagent_caps(self):
        launched = {}

        def fake(command, cwd, timeout, stdout_path, stderr_path, env):
            launched[command[0]] = (command, env)
            Path(stdout_path).write_text("")
            return {"seconds": 1.0, "exit_code": 0, "timed_out": False, "cleanup_error": None}
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(hb, "run_subprocess_capturing", side_effect=fake), \
                unittest.mock.patch.object(hb, "native_configured_defaults", return_value={}), \
                unittest.mock.patch.object(hb, "native_resolved_metadata", return_value=[]):
            claude = {}
            for model, result in (("haiku", claude), ("luna", {})):
                condition = build_arm(model, "native", allow_delegation=True)["condition"]
                hb.run_native_trial(condition, "task", directory, 5, directory, result, extra_env={"TMPDIR": "/t"})
        command, env = launched["claude"]
        self.assertEqual(env["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "4")
        self.assertEqual(claude["env_isolation"]["extra_vars"], ["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS", "TMPDIR"])
        command, env = launched["codex"]
        self.assertIn("agents.max_concurrent_threads_per_session=4", command)
        self.assertIn("features.multi_agent_v2.max_concurrent_threads_per_session=4", command)
        self.assertNotIn("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS", env)


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
    state["operator_config"] = open(os.path.join(os.environ["AETHER_CONFIG_DIR"], "config.toml")).read()
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
            self.assertEqual(state["operator_config"], "[scheduler]\nmax_concurrent_turns = 2\n",
                             "Archon seeds its Aether with the orchestrator and the stage agent as the turn cap")
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
