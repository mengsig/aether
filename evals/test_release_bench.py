"""Release bench: statistics, gate, scheduling, budget, selection and frozen sets, without a model."""
import random
import tempfile
import unittest
from pathlib import Path

import release_bench_stats as stats
from harness_bench_treatments import native_command
from release_bench_arms import build_arm, parse_arms
from release_bench_calibrate import (
    SELECTION_DEFAULTS,
    acceptance,
    eligible,
    select,
    summary,
    task_rates,
)
from release_bench_report import gate_verdicts, overall_verdict
from release_bench_tasks import load_set, validate_spec, write_set
from release_bench_trial import harness_failure, infra_error, record


def rec(task, arm, passed, cost=0.01, repeat=0, infra=None, fraction=None, template="t"):
    model, harness = arm.split("/")
    return {"task": task, "arm": arm, "model": model, "harness": harness, "repeat": repeat, "passed": passed,
            "fraction": (1.0 if passed else 0.5) if fraction is None else fraction, "cost_usd": cost,
            "infra_error": infra, "template": template, "family": "f", "checks_passed": 1, "checks_total": 1,
            "seconds": 1}


def paired(outcomes_old, outcomes_new, cost_old=0.01, cost_new=0.01, repeat=0):
    records = []
    for index, (old, new) in enumerate(zip(outcomes_old, outcomes_new)):
        records += [rec(f"task-{index}", "haiku/old", old, cost_old, repeat),
                    rec(f"task-{index}", "haiku/new", new, cost_new, repeat)]
    return records


def plan(tasks, repeats=1, arms=("haiku/old", "haiku/new")):
    return {"arms": list(arms), "tasks": [f"task-{index}" for index in range(tasks)], "repeats": repeats}


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
                   rec("a", "haiku/old", True, cost=0.03, repeat=3)]
        row = stats.per_task(records, "haiku/old")["a"]
        self.assertAlmostEqual(row["pass"], 2 / 3)
        self.assertEqual(row["n"], 3)
        self.assertAlmostEqual(row["cost"], 0.05 / 3)


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
        self.assertIn("haiku/new infra", verdict["reasons"][0])
        self.assertEqual(verdict["infra_rate"]["haiku/old"], 0)

    def test_outages_on_one_arm_alone_are_inconclusive(self):
        # 3 of 40 trials overall stays under 10%, but 3 of 20 on the candidate does not.
        records = paired([True] * 20, [True] * 20)
        for item in records[1:7:2]:
            item["infra_error"] = "provider: 529"
        self.assertEqual(stats.gate(records, "haiku/old", "haiku/new")["verdict"], "INCONCLUSIVE")

    def test_candidate_only_crashes_fail_the_gate(self):
        old = [True] * 40
        new = [index % 4 != 0 for index in range(40)]
        records = paired(old, new)
        for item in records:
            if item["arm"] == "haiku/new" and not item["passed"]:
                item["harness_failure"] = "no model output: exit 101"
        verdict = stats.gate(records, "haiku/old", "haiku/new", expected=plan(40))
        self.assertEqual(verdict["verdict"], "FAIL", verdict["reasons"])
        self.assertEqual(verdict["infra_rate"]["haiku/new"], 0)


class IncompleteMatrixTests(unittest.TestCase):
    OLD, NEW = "haiku/old", "haiku/new"

    def test_a_complete_matrix_passes(self):
        records = paired([True, False] * 15, [True, False] * 15)
        self.assertEqual(stats.gate(records, self.OLD, self.NEW, expected=plan(30))["verdict"], "PASS")

    def test_a_budget_stop_is_inconclusive(self):
        records = paired([True, False] * 15, [True, False] * 15)[:-6]
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(30))
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("haiku/old ran 27 of 30", " ".join(verdict["reasons"]))

    def test_a_partial_repeat_is_inconclusive(self):
        records = paired([True, False] * 15, [True, False] * 15) + paired([True] * 10, [True] * 10, repeat=1)
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(30, repeats=2))
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("40 of 60", verdict["reasons"][0])

    def test_a_missing_tier_is_inconclusive_overall(self):
        records = paired([True, False] * 15, [True, False] * 15)
        expected = plan(30, arms=("haiku/old", "haiku/new", "luna/old", "luna/new"))
        verdicts = gate_verdicts(records, ["haiku", "luna"], {}, expected)
        self.assertEqual(verdicts["haiku"]["verdict"], "PASS")
        self.assertEqual(verdicts["luna"]["verdict"], "INCONCLUSIVE")
        self.assertEqual(overall_verdict(verdicts), "INCONCLUSIVE")

    def test_an_incomplete_failure_fails_only_under_the_sequential_plan(self):
        records = paired([True] * 30, [False] * 30)
        settings = {"confidence": stats.sequential_confidence(0.95)}
        verdict = stats.gate(records, self.OLD, self.NEW, settings, expected=plan(60))
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("would fail", verdict["warnings"][0])
        settings["sequential"] = True
        self.assertEqual(stats.gate(records, self.OLD, self.NEW, settings, expected=plan(60))["verdict"], "FAIL")


class PricingCoverageTests(unittest.TestCase):
    OLD, NEW = "haiku/old", "haiku/new"

    def test_no_prices_at_all_block_the_pass(self):
        records = paired([True] * 30, [True] * 30, cost_old=None, cost_new=None)
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(30))
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("cost could not be compared", verdict["reasons"][0])

    def test_prices_missing_on_one_arm_block_the_pass(self):
        records = paired([True] * 30, [True] * 30)
        for item in records:
            if item["arm"] == self.NEW and item["task"] in {f"task-{index}" for index in range(5)}:
                item["cost_usd"] = None
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(30))
        self.assertEqual(verdict["verdict"], "INCONCLUSIVE")
        self.assertIn("83% of paired tasks are priced", verdict["reasons"][0])

    def test_an_unpriced_repeat_unprices_its_task(self):
        records = [rec("a", self.OLD, True, cost=0.01, repeat=0), rec("a", self.OLD, True, cost=None, repeat=1)]
        self.assertIsNone(stats.per_task(records, self.OLD)["a"]["cost"])

    def test_missing_prices_keep_an_established_fail(self):
        records = paired([True] * 30, [False] * 30, cost_old=None, cost_new=None)
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(30))
        self.assertEqual(verdict["verdict"], "FAIL")
        self.assertIn("pass rate", verdict["reasons"][0])

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
        records[0]["infra_error"] = "provider: 529"
        self.assertEqual(eligible(task_rates(records), {**SELECTION_DEFAULTS, "min_repeats": 3}), [])

    def test_only_tasks_inside_the_haiku_band_are_eligible(self):
        measured = task_rates(self._records({"all-0": ("a", 1, 1), "none-0": ("a", 0, 0), "split-0": ("a", 1, 0),
                                             "mid-0": ("a", 0.5, 0.5), "low-0": ("a", 0.25, 0),
                                             "high-0": ("a", 0.75, 1)}))
        self.assertEqual(eligible(measured, SELECTION_DEFAULTS), ["high-0", "low-0", "mid-0"])

    def test_acceptance_needs_the_size_and_both_targets(self):
        rates = {f"t-{index}": ("t", 0.5, 0.5 if index == 0 else 0.25) for index in range(8)}
        measured = task_rates(self._records(rates))
        chosen = sorted(rates)
        self.assertTrue(acceptance(measured, chosen, {"min_size": 8})["accepted"])
        small = acceptance(measured, chosen, {"min_size": 9})
        self.assertFalse(small["accepted"])
        self.assertIn("8 in-band tasks", small["reasons"][0])
        easy = task_rates(self._records({task: ("t", 0.75, 0.75) for task in rates}))
        verdict = acceptance(easy, chosen, {"min_size": 8})
        self.assertEqual(len(verdict["reasons"]), 2)
        self.assertFalse(acceptance(measured, [], {})["accepted"])


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

    def test_line_separators_inside_strings_survive_a_round_trip(self):
        spec = {**self._spec(), "files": {"a.py": "s = '\u2028\x85\x0c\x1c'\n"}}
        with tempfile.TemporaryDirectory() as directory:
            write_set(Path(directory, "v1"), [spec], {})
            self.assertEqual(load_set(Path(directory, "v1"))[1][0]["files"], spec["files"])

    def test_frozen_set_round_trips_and_detects_tampering(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory, "v1")
            manifest = write_set(target, [self._spec("tiny-0"), self._spec("tiny-1")], {"note": "test"})
            loaded, specs = load_set(target)
            self.assertEqual(loaded["set_sha256"], manifest["set_sha256"])
            self.assertEqual([spec["id"] for spec in specs], ["tiny-0", "tiny-1"])
            with self.assertRaises(FileExistsError):
                write_set(target, [self._spec()], {})
            text = (target / "tasks.jsonl").read_text(encoding="utf-8").replace("x = 2", "x = 3")
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

    GRADE = {"passed": 4, "total": 4, "failed": [], "error": None}

    def test_a_pass_needs_every_check_and_no_failure(self):
        self.assertTrue(record(self.SPEC, self.ARM, 0, self._row(), self.GRADE, 0.01, 100)["passed"])
        broken = record(self.SPEC, self.ARM, 0, self._row(bench_error="OSError: disk"), self.GRADE, None, None)
        self.assertFalse(broken["passed"])
        self.assertIn("bench", broken["infra_error"])
        partial = record(self.SPEC, self.ARM, 0, self._row(), {**self.GRADE, "passed": 3}, 0.01, 100)
        self.assertEqual((partial["passed"], partial["fraction"]), (False, 0.75))

    def test_a_candidate_crash_is_a_counted_failure(self):
        row = self._row(workflow_status="failed", usage_report={"usage": {"output_tokens": 0}}, exit_code=101,
                        workflow_error="thread 'main' panicked")
        self.assertIsNone(infra_error(row, "new"))
        self.assertIn("no model output", harness_failure(row, "new"))
        crashed = record(self.SPEC, self.ARM, 0, row, self.GRADE, 0.0, 0)
        self.assertEqual((crashed["passed"], crashed["infra_error"]), (False, None))
        self.assertIn("cleanup_error", harness_failure(self._row(cleanup_error="socket left"), "old"))

    def test_only_outages_and_native_cli_failures_are_infra(self):
        silent = self._row(workflow_status="failed", usage_report={"usage": {"output_tokens": 0}}, exit_code=1)
        self.assertIn("no model output", infra_error(silent, "native"))
        self.assertIsNone(infra_error(self._row(workflow_status="failed"), "native"))
        outage = self._row(workflow_status="failed", workflow_error="HTTP 529: Overloaded")
        self.assertIn("provider", infra_error(outage, "new"))
        self.assertIsNone(harness_failure(outage, "native"))

    def test_a_spend_stop_is_a_harness_failure_on_every_arm(self):
        row = self._row(spend_stop="spend $0.51 reached the trial allowance $0.50")
        self.assertIsNone(infra_error(row, "native"))
        self.assertIn("spend guard", harness_failure(row, "native"))
        self.assertFalse(record(self.SPEC, self.ARM, 0, row, self.GRADE, 0.52, 9)["passed"])


if __name__ == "__main__":
    unittest.main()
