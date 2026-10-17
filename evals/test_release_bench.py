"""Release bench: statistics, gate, scheduling, budget, selection and frozen sets, without a model."""
import json
import random
import tempfile
import unittest
from pathlib import Path

import release_bench_stats as stats
from harness_bench_treatments import native_command
from release_bench_arms import build_arm, parse_arms
from release_bench_calibrate import (
    SELECTION_DEFAULTS,
    eligible,
    select,
    summary,
    task_rates,
)
from release_bench_pool import Gates, read_results, run_pool, schedule
from release_bench_report import overall_verdict
from release_bench_tasks import load_set, validate_spec, write_set
from release_bench_trial import infra_error, record


def rec(task, arm, passed, cost=0.01, repeat=0, infra=None, fraction=None, template="t"):
    model, harness = arm.split("/")
    return {"task": task, "arm": arm, "model": model, "harness": harness, "repeat": repeat, "passed": passed,
            "fraction": (1.0 if passed else 0.5) if fraction is None else fraction, "cost_usd": cost,
            "infra_error": infra, "template": template, "family": "f", "checks_passed": 1, "checks_total": 1,
            "seconds": 1}


def paired(outcomes_old, outcomes_new, cost_old=0.01, cost_new=0.01):
    records = []
    for index, (old, new) in enumerate(zip(outcomes_old, outcomes_new)):
        records += [rec(f"task-{index}", "haiku/old", old, cost_old), rec(f"task-{index}", "haiku/new", new, cost_new)]
    return records


class McNemarAndBootstrapTests(unittest.TestCase):
    def test_exact_mcnemar_values(self):
        self.assertEqual(stats.mcnemar_exact(0, 0), 1.0)
        self.assertAlmostEqual(stats.mcnemar_exact(0, 10), 2 / 1024)
        self.assertAlmostEqual(stats.mcnemar_exact(1, 9), 2 * 11 / 1024)
        self.assertEqual(stats.mcnemar_exact(3, 7), stats.mcnemar_exact(7, 3))
        self.assertEqual(stats.mcnemar_exact(5, 5), 1.0)

    def test_discordant_pairs_match_on_task_and_repeat(self):
        records = paired([True, True, False, False], [True, False, True, False])
        records.append(rec("task-9", "haiku/old", True))
        self.assertEqual(stats.discordant(records, "haiku/old", "haiku/new"), (1, 1, 4))

    def test_bootstrap_is_seeded_and_brackets_the_point(self):
        rng = random.Random(1)
        pairs = [rng.random() for _ in range(50)]
        first = stats.bootstrap(pairs, stats.mean, seed=3)
        self.assertEqual(first, stats.bootstrap(pairs, stats.mean, seed=3))
        point, low, high = first
        self.assertLess(low, point)
        self.assertLess(point, high)
        self.assertEqual(stats.bootstrap([2.0] * 10, stats.mean)[1:], (2.0, 2.0))

    def test_percentile_interpolates(self):
        self.assertEqual(stats.percentile([0, 10], 0.5), 5)
        self.assertEqual(stats.percentile([1, 2, 3], 1), 3)

    def test_repeats_average_per_task_and_infra_errors_are_excluded(self):
        records = [rec("a", "haiku/old", True, repeat=0), rec("a", "haiku/old", False, repeat=1),
                   rec("a", "haiku/old", False, repeat=2, infra="harness_error: x"),
                   rec("a", "haiku/old", True, cost=None, repeat=3)]
        row = stats.per_task(records, "haiku/old")["a"]
        self.assertAlmostEqual(row["pass"], 2 / 3)
        self.assertEqual(row["n"], 3)
        self.assertAlmostEqual(row["cost"], 0.01)


class GateTests(unittest.TestCase):
    def test_overall_verdict(self):
        tiers = {name: {"verdict": name} for name in ("PASS", "FAIL", "INCONCLUSIVE")}
        self.assertEqual(overall_verdict({"a": tiers["PASS"], "b": tiers["PASS"]}), "PASS")
        self.assertEqual(overall_verdict({"a": tiers["INCONCLUSIVE"], "b": tiers["FAIL"]}), "FAIL")
        self.assertEqual(overall_verdict({"a": tiers["PASS"], "b": tiers["INCONCLUSIVE"]}), "INCONCLUSIVE")
        self.assertEqual(overall_verdict({}), "INCONCLUSIVE")

    def test_identical_arms_pass(self):
        outcomes = [index % 2 == 0 for index in range(40)]
        verdict = stats.gate(paired(outcomes, outcomes), "haiku/old", "haiku/new")
        self.assertEqual(verdict["verdict"], "PASS", verdict["reasons"])
        self.assertEqual(verdict["comparison"]["pass"][0], 0)

    def test_noisy_equal_arms_pass(self):
        rng = random.Random(7)
        old = [rng.random() < 0.5 for _ in range(100)]
        new = [rng.random() < 0.5 for _ in range(100)]
        self.assertEqual(stats.gate(paired(old, new), "haiku/old", "haiku/new")["verdict"], "PASS")

    def test_a_clear_accuracy_regression_fails(self):
        old = [True] * 40
        new = [index % 3 == 0 for index in range(40)]
        verdict = stats.gate(paired(old, new), "haiku/old", "haiku/new")
        self.assertEqual(verdict["verdict"], "FAIL")
        self.assertIn("pass rate", verdict["reasons"][0])

    def test_a_small_regression_within_the_margin_passes(self):
        old = [True] * 100
        new = [index != 0 for index in range(100)]
        self.assertEqual(stats.gate(paired(old, new), "haiku/old", "haiku/new")["verdict"], "PASS")

    def test_a_clear_cost_regression_fails(self):
        outcomes = [True] * 30
        verdict = stats.gate(paired(outcomes, outcomes, cost_new=0.02), "haiku/old", "haiku/new")
        self.assertEqual(verdict["verdict"], "FAIL")
        self.assertIn("cost", verdict["reasons"][0])

    def test_too_few_tasks_or_too_many_infra_errors_are_inconclusive(self):
        few = paired([True] * 5, [True] * 5)
        self.assertEqual(stats.gate(few, "haiku/old", "haiku/new")["verdict"], "INCONCLUSIVE")
        broken = paired([True] * 30, [True] * 30)
        broken += [rec(f"x-{index}", "haiku/new", False, infra="harness_error: x") for index in range(10)]
        verdict = stats.gate(broken, "haiku/old", "haiku/new")
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("infra", verdict["reasons"][0])

    def test_interim_looks_count_once_and_split_the_error_rate(self):
        self.assertAlmostEqual(stats.sequential_confidence(0.95), 1 - 0.05 / 3)
        records = paired([True] * 30, [True] * 30)
        self.assertEqual(stats.reached_looks(records, "haiku/old", "haiku/new", 100), 0)
        self.assertEqual(stats.reached_looks(records, "haiku/old", "haiku/new", 50), 1)
        self.assertEqual(stats.reached_looks(records, "haiku/old", "haiku/new", 40), 2)
        settings = {"confidence": stats.sequential_confidence(0.95)}
        self.assertIsNone(stats.interim_fail(records, "haiku/old", "haiku/new", settings))
        failing = paired([True] * 30, [False] * 30)
        self.assertEqual(stats.interim_fail(failing, "haiku/old", "haiku/new", settings)["verdict"], "FAIL")


class ScheduleAndPoolTests(unittest.TestCase):
    def test_schedule_keeps_a_task_s_arms_together_and_rotates_them(self):
        arms = ["haiku/native", "haiku/old", "haiku/new"]
        order = schedule(["a", "b", "c", "d"], arms, 2, seed=1)
        self.assertEqual(len(order), 24)
        self.assertEqual(len(set(order)), 24)
        blocks = [order[index:index + 3] for index in range(0, 24, 3)]
        self.assertTrue(all(len({(task, repeat) for task, repeat, _ in block}) == 1 for block in blocks))
        self.assertGreater(len({block[0][2] for block in blocks}), 1)

    def _pool(self, directory, order, budget=10.0, cost=0.01):
        def run_one(triple):
            task, repeat, arm = triple
            return {**rec(task, arm, True, cost=cost, repeat=repeat), "checks_passed": 1, "checks_total": 1}
        gates = Gates(load_max=10_000, mem_min_gb=0, budget_usd=budget, unpriced_usd={"haiku": 0.5, "luna": 0.4})
        return run_pool(order, run_one, lambda triple: triple[2].split("/")[0], jobs=3, gates=gates,
                        results_path=str(Path(directory, "results.jsonl")), log=lambda _line: None)

    def test_pool_records_every_trial_and_resumes(self):
        order = schedule(["a", "b"], ["haiku/old", "haiku/new"], 2, seed=0)
        with tempfile.TemporaryDirectory() as directory:
            records, stopped = self._pool(directory, order[:5])
            self.assertIsNone(stopped)
            self.assertEqual(len(records), 5)
            records, _ = self._pool(directory, order)
            self.assertEqual(len(records), 8)
            self.assertEqual(len(read_results(str(Path(directory, "results.jsonl")))), 8)

    def test_pool_stops_admitting_at_the_budget(self):
        order = schedule([f"t{index}" for index in range(20)], ["haiku/old"], 1, seed=0)
        with tempfile.TemporaryDirectory() as directory:
            records, stopped = self._pool(directory, order, budget=1.2, cost=0.3)
        self.assertIn("budget", stopped)
        self.assertLessEqual(sum(item["cost_usd"] for item in records), 1.2 + 0.5)
        self.assertGreaterEqual(len(records), 2)

    def test_torn_results_line_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "results.jsonl")
            path.write_text(json.dumps(rec("a", "haiku/old", True)) + "\n{\"task\": \n")
            with self.assertRaises(ValueError):
                read_results(str(path))

    def test_budget_reservations_settle_to_actual_spend(self):
        gates = Gates(load_max=1, mem_min_gb=0, budget_usd=1.0, unpriced_usd={"haiku": 0.5, "luna": 0.4})
        self.assertTrue(gates.reserve("haiku"))
        self.assertTrue(gates.reserve("haiku"))
        self.assertFalse(gates.reserve("luna"))
        gates.settle("haiku", 0.05)
        gates.settle("haiku", None)
        self.assertAlmostEqual(gates.spent_usd, 0.55)
        self.assertAlmostEqual(gates.reserved_usd, 0.0)


class SelectionTests(unittest.TestCase):
    def _records(self, rates):
        records = []
        for task, (template, haiku, luna) in rates.items():
            for repeat in range(4):
                records.append({**rec(task, "haiku/native", repeat < haiku * 4, repeat=repeat), "template": template})
                records.append({**rec(task, "luna/native", repeat < luna * 4, repeat=repeat), "template": template})
        return records

    def test_selection_drops_saturated_tasks_caps_templates_and_nears_targets(self):
        rng = random.Random(3)
        rates = {f"{template}-{seed}": (template, rng.choice([0, 0.25, 0.5, 0.75, 1]), rng.choice([0, 0.25, 0.5]))
                 for template in "abcdefg" for seed in range(30)}
        measured = task_rates(self._records(rates))
        settings = {"min_size": 40, "max_size": 60}
        chosen = select(measured, settings)
        self.assertTrue(set(chosen) <= set(eligible(measured, SELECTION_DEFAULTS)))
        self.assertFalse([task for task in chosen if rates[task][1] == rates[task][2] in (0, 1)])
        self.assertTrue(40 <= len(chosen) <= 60)
        result = summary(measured, chosen)
        self.assertLessEqual(max(result["templates"].values()), 12)
        self.assertAlmostEqual(result["haiku_pass"], 0.5, delta=0.05)

    def test_infra_errors_make_a_task_ineligible(self):
        records = self._records({"x-0": ("x", 0.5, 0.25)})
        records[0]["infra_error"] = "harness_error: boom"
        self.assertEqual(eligible(task_rates(records), {"min_repeats": 3}), [])

    def test_saturated_tasks_are_ineligible(self):
        measured = task_rates(self._records({"all-0": ("a", 1, 1), "none-0": ("a", 0, 0), "split-0": ("a", 1, 0),
                                             "mid-0": ("a", 0.5, 0.5)}))
        self.assertEqual(eligible(measured, SELECTION_DEFAULTS), ["mid-0", "split-0"])


class ArmTests(unittest.TestCase):
    def test_arm_shorthands(self):
        self.assertEqual(len(parse_arms("all")), 6)
        self.assertEqual(parse_arms("luna"), ["luna/native", "luna/old", "luna/new"])
        self.assertEqual(parse_arms("haiku/new", archon=True), ["haiku/new", "haiku/archon"])
        with self.assertRaises(AssertionError):
            parse_arms("opus/native")

    def test_every_arm_is_capped(self):
        native = build_arm("haiku", "native")["condition"]
        self.assertEqual(native["max_budget_usd"], 0.5)
        command = native_command(native, ["claude", "-p", "--model", "claude-haiku-5-5", "task"])
        self.assertEqual(command[2:4], ["--max-budget-usd", "0.5"])
        self.assertEqual(build_arm("haiku", "old")["condition"]["budget"], {"cost_budget": 0.5})
        self.assertIn("token_budget", build_arm("luna", "new")["condition"]["budget"])
        self.assertEqual(build_arm("luna", "new")["binary_role"], "new")


class TaskSetTests(unittest.TestCase):
    def _spec(self, task_id="tiny-0"):
        return {"id": task_id, "family": "f", "template": task_id.split("-")[0], "seed": int(task_id.split("-")[1]),
                "prompt": "fix it", "files": {"a.py": "x = 1\n"}, "history": [], "tags": {}, "meta": {"k": 1},
                "hidden": "import unittest\n", "checks": 2, "solution": {"a.py": "x = 2\n"}}

    def test_frozen_set_round_trips_and_detects_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory, "v1")
            manifest = write_set(target, [self._spec("tiny-0"), self._spec("tiny-1")], {"note": "test"})
            loaded, specs = load_set(target)
            self.assertEqual(loaded["set_sha256"], manifest["set_sha256"])
            self.assertEqual([spec["id"] for spec in specs], ["tiny-0", "tiny-1"])
            with self.assertRaises(FileExistsError):
                write_set(target, [self._spec()], {})
            text = (target / "tasks.jsonl").read_text().replace("x = 2", "x = 3")
            (target / "tasks.jsonl").write_text(text)
            with self.assertRaises(ValueError):
                load_set(target)

    def test_meta_holds_only_scalars(self):
        spec = self._spec()
        spec["meta"] = {"nested": {"no": 1}}
        with self.assertRaises(ValueError):
            validate_spec(spec)


class TrialRecordTests(unittest.TestCase):
    SPEC = {"id": "tiny-0", "family": "f", "template": "tiny", "checks": 4}
    ARM = {"key": "haiku/new", "model": "haiku", "harness": "new"}

    def _row(self, **extra):
        return {"trial_dir": "/x", "started_at": "2026-01-01T00:00:00Z", "workflow_status": "completed",
                "usage_report": {"usage": {"output_tokens": 10}}, **extra}

    def test_a_pass_needs_every_check_and_no_infra_error(self):
        grade = {"passed": 4, "total": 4, "failed": [], "error": None}
        self.assertTrue(record(self.SPEC, self.ARM, 0, self._row(), grade, 0.01, 100)["passed"])
        broken = record(self.SPEC, self.ARM, 0, self._row(harness_error="boom"), grade, None, None)
        self.assertFalse(broken["passed"])
        self.assertIn("harness_error", broken["infra_error"])
        partial = record(self.SPEC, self.ARM, 0, self._row(), {**grade, "passed": 3}, 0.01, 100)
        self.assertEqual((partial["passed"], partial["fraction"]), (False, 0.75))

    def test_a_failed_workflow_without_model_output_is_infra(self):
        row = self._row(workflow_status="failed", usage_report={"usage": {"output_tokens": 0}}, exit_code=1)
        self.assertIn("no model output", infra_error(row))
        self.assertIsNone(infra_error(self._row(workflow_status="failed")))


if __name__ == "__main__":
    unittest.main()
