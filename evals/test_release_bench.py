"""Release bench: statistics, gate, scheduling, budget, selection and frozen sets, without a model."""
import argparse
import random
import tempfile
import unittest
from pathlib import Path

import release_bench_stats as stats
from harness_bench_treatments import native_command
from release_bench import _band, carried
from release_bench_arms import build_arm, parse_arms
from release_bench_calibrate import (
    SELECTION_DEFAULTS,
    acceptance,
    eligible,
    release_minutes,
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


class RepeatPairingTests(unittest.TestCase):
    OLD, NEW = "haiku/old", "haiku/new"

    def _identical(self, tasks=20, repeats=10):
        """Both arms pass repeats 0-4 and fail 5-9 of every task."""
        return [rec(f"task-{index}", arm, repeat < 5, repeat=repeat)
                for index in range(tasks) for repeat in range(repeats) for arm in (self.OLD, self.NEW)]

    def test_outages_on_different_repeats_do_not_fake_a_regression(self):
        # Old loses a failing repeat and new a passing one: averaging each arm's survivors would show -11%.
        records = self._identical()
        for item in records:
            if (item["arm"], item["repeat"]) in ((self.OLD, 9), (self.NEW, 0)):
                item["infra_error"] = "provider: 529"
        verdict = stats.gate(records, self.OLD, self.NEW, expected=plan(20, repeats=10))
        self.assertEqual(verdict["verdict"], "PASS", verdict["reasons"])
        self.assertEqual(verdict["comparison"]["pass"], (0, 0, 0))
        self.assertEqual(verdict["comparison"]["pairs"], 160)
        self.assertEqual(verdict["comparison"]["old_only"] + verdict["comparison"]["new_only"], 0)

    def test_cost_is_compared_on_the_matched_repeats_only(self):
        records = self._identical(tasks=1, repeats=2)
        for item in records:
            if item["arm"] == self.OLD and item["repeat"] == 1:
                item["cost_usd"], item["infra_error"] = 0.05, "provider: 529"
            if item["arm"] == self.NEW and item["repeat"] == 1:
                item["cost_usd"] = None
        (old, new), = stats._paired_tasks(records, self.OLD, self.NEW)
        self.assertEqual((old["n"], new["n"]), (1, 1))
        self.assertEqual((old["cost"], new["cost"]), (0.01, 0.01))

    def test_a_task_with_no_matched_repeat_is_not_paired(self):
        records = [rec("a", self.OLD, True, repeat=0), rec("a", self.NEW, True, repeat=1)]
        self.assertEqual(stats.compare(records, self.OLD, self.NEW)["tasks"], 0)
        self.assertEqual(stats.discordant(records, self.OLD, self.NEW), (0, 0, 0))


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


class ReferenceArmTests(unittest.TestCase):
    """An overall PASS needs every planned arm to finish, the native and Archon references included."""

    def _verdict(self, records, arms):
        expected = plan(30, arms=arms)
        verdicts = gate_verdicts(records, ["haiku"], {}, expected)
        return verdicts["haiku"]["verdict"], overall_verdict(verdicts, stats.matrix_gaps(records, expected))

    def test_a_missing_native_reference_blocks_the_pass(self):
        records = paired([True, False] * 15, [True, False] * 15)
        arms = ("haiku/native", "haiku/old", "haiku/new")
        self.assertEqual(self._verdict(records, arms), ("PASS", "INCONCLUSIVE"))
        records += [rec(f"task-{index}", "haiku/native", True) for index in range(29)]
        self.assertEqual(self._verdict(records, arms), ("PASS", "INCONCLUSIVE"))
        self.assertEqual(stats.matrix_gaps(records, plan(30, arms=arms)),
                         ["haiku/native ran 29 of 30 planned trials"])
        records.append(rec("task-29", "haiku/native", False))
        self.assertEqual(self._verdict(records, arms), ("PASS", "PASS"))

    def test_a_missing_archon_arm_blocks_the_pass(self):
        records = paired([True, False] * 15, [True, False] * 15)
        records += [rec(f"task-{index}", "haiku/archon", True) for index in range(10)]
        self.assertEqual(self._verdict(records, ("haiku/old", "haiku/new", "haiku/archon")),
                         ("PASS", "INCONCLUSIVE"))

    def test_a_plan_without_references_can_pass(self):
        records = paired([True, False] * 15, [True, False] * 15)
        self.assertEqual(self._verdict(records, ("haiku/old", "haiku/new")), ("PASS", "PASS"))

    def test_a_failure_stands_while_a_reference_is_missing(self):
        records = paired([True] * 30, [False] * 30)
        self.assertEqual(self._verdict(records, ("haiku/native", "haiku/old", "haiku/new")), ("FAIL", "FAIL"))


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
        rates = {f"t-{index}": (f"t{index}", 0.5, 0.5 if index == 0 else 0.25) for index in range(8)}
        measured = task_rates(self._records(rates))
        chosen = sorted(rates)
        self.assertTrue(acceptance(measured, chosen, {"min_size": 8})["accepted"])
        small = acceptance(measured, chosen, {"min_size": 9})
        self.assertFalse(small["accepted"])
        self.assertIn("8 in-band tasks", small["reasons"][0])
        easy = task_rates(self._records({task: (template, 0.75, 0.75) for task, (template, _, _) in rates.items()}))
        verdict = acceptance(easy, chosen, {"min_size": 8})
        self.assertEqual(len(verdict["reasons"]), 2)
        self.assertFalse(acceptance(measured, [], {})["accepted"])


class TemplateShareTests(unittest.TestCase):
    """No template holds more than 20% of the final set, however far selection trims it."""

    def _rates(self, templates):
        """Tasks of each template at the given Haiku rates; every task sits on the Luna target."""
        return {f"{template}-{index}": {"template": template, "family": "f", "infra": 0, "cost": 0.0,
                                        "haiku": haiku, "luna": 0.3, "haiku_n": 4, "luna_n": 4,
                                        "haiku_seconds": 1.0, "luna_seconds": 1.0}
                for template, rates in templates.items() for index, haiku in enumerate(rates)}

    def test_the_cap_holds_against_the_final_size_not_the_maximum(self):
        # 12 "big" tasks fit 20% of max_size 60, but the set stays at 44, where 20% is 8.
        rates = self._rates({"big": [0.5] * 8 + [0.25, 0.75] * 2, **{name: [0.5] * 8 for name in "abcd"}})
        chosen = select(rates, {"min_size": 40, "max_size": 60})
        counts = summary(rates, chosen)["templates"]
        self.assertLessEqual(max(counts.values()), 0.2 * len(chosen))
        self.assertEqual(counts["big"], 8)
        self.assertEqual({rates[task]["haiku"] for task in chosen if task.startswith("big")}, {0.5},
                         "the template's tasks farthest from the Haiku target go first")
        self.assertTrue(acceptance(rates, chosen, {"min_size": 40})["accepted"])

    def test_too_few_templates_shrink_the_set_and_acceptance_says_so(self):
        # Three templates can never hold 20% each, so capping ends at one task of each.
        rates = self._rates({"big": [0.5] * 20, "a": [0.5] * 10, "b": [0.5] * 10})
        chosen = select(rates, {"min_size": 40, "max_size": 60})
        self.assertEqual(summary(rates, chosen)["templates"], {"a": 1, "b": 1, "big": 1})
        verdict = acceptance(rates, chosen, {"min_size": 40})
        self.assertFalse(verdict["accepted"])
        self.assertEqual(verdict["reasons"][:2], ["3 in-band tasks (< 40)", "template big holds 1 of 3 tasks (> 20%)"])
        five = self._rates({name: [0.5] * 10 for name in ("big", "a", "b", "c", "d")})
        self.assertEqual(len(select(five, {"min_size": 40, "max_size": 60})), 50)

    def test_acceptance_refuses_a_set_one_template_dominates(self):
        rates = self._rates({"big": [0.5] * 3, **{name: [0.5] for name in "abcdefg"}})
        verdict = acceptance(rates, sorted(rates), {"min_size": 10})
        self.assertFalse(verdict["accepted"])
        self.assertEqual(verdict["reasons"], ["template big holds 3 of 10 tasks (> 20%)"])
        balanced = self._rates({name: [0.5, 0.5] for name in "abcde"})
        self.assertTrue(acceptance(balanced, sorted(balanced), {"min_size": 10})["accepted"])


class ReleaseTimeTests(unittest.TestCase):
    def _rates(self, seconds):
        return {f"t-{index}": {"template": f"t{index % 8}", "family": "f", "infra": 0, "cost": 0.0,
                               "haiku": 0.5, "luna": 0.3, "haiku_n": 4, "luna_n": 4,
                               "haiku_seconds": haiku, "luna_seconds": luna}
                for index, (haiku, luna) in enumerate(seconds)}

    def test_release_minutes_count_three_arms_per_tier(self):
        rates = self._rates([(60, 20), (120, 40)])
        self.assertAlmostEqual(release_minutes(rates, sorted(rates), jobs=1), (180 * 3 + 60 * 3.9) / 60)
        self.assertAlmostEqual(release_minutes(rates, sorted(rates), jobs=6),
                               release_minutes(rates, sorted(rates), jobs=1) / 6)
        self.assertIsNone(release_minutes({"x": {"haiku_seconds": 5}}, ["x"], jobs=6))

    def test_selection_drops_the_slowest_tasks_until_the_run_fits(self):
        rates = self._rates([(100, 40)] * 80 + [(400, 100)] * 10)
        # 80 fast tasks need 101 min on 6 workers; each slow one adds 4.4 min.
        chosen = select(rates, {"min_size": 80, "max_size": 120, "max_release_minutes": 110})
        self.assertLessEqual(release_minutes(rates, chosen, 6), 110)
        self.assertEqual(len(chosen), 81)
        self.assertEqual(sum(1 for task in chosen if rates[task]["haiku_seconds"] == 400), 1)
        floor = select(rates, {"min_size": 85, "max_size": 120, "max_release_minutes": 60})
        self.assertEqual(len(floor), 85, "trimming for time stops at min_size")

    def test_a_run_too_long_or_of_unknown_length_is_not_accepted(self):
        slow = self._rates([(300, 100)] * 80)
        verdict = acceptance(slow, sorted(slow), {})
        self.assertFalse(verdict["accepted"])
        self.assertIn("min on 6 workers", verdict["reasons"][0])
        self.assertTrue(acceptance(slow, sorted(slow), {"max_release_minutes": 400})["accepted"])
        untimed = {task: {**row, "haiku_seconds": None} for task, row in slow.items()}
        self.assertIn("no calibration timing", acceptance(untimed, sorted(untimed), {"max_release_minutes": 400})
                      ["reasons"][0])
        self.assertEqual(len(select(untimed, {})), 80)


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

    def test_a_carried_set_keeps_its_recorded_rates_minus_recalibrated_templates(self):
        specs = [self._spec("tiny-0"), self._spec("tiny-1"), {**self._spec("other-0"), "template": "other"}]
        rates = {spec["id"]: {"haiku": 1.0, "luna": 0.5, "haiku_n": 2, "luna_n": 2} for spec in specs}
        with tempfile.TemporaryDirectory() as directory:
            write_set(Path(directory, "v1"), specs, {"calibration": rates})
            kept, carried_rates = carried(Path(directory, "v1"), {"other"})
            self.assertEqual([spec["id"] for spec in kept], ["tiny-0", "tiny-1"])
            self.assertEqual(carried_rates["tiny-1"]["luna"], 0.5)
            self.assertEqual(task_rates([rec("x-0", "haiku/native", True)]).keys() & carried_rates.keys(), set())
            write_set(Path(directory, "bare"), specs, {})
            with self.assertRaises(SystemExit):
                carried(Path(directory, "bare"), None)

    def test_a_band_is_two_ordered_fractions(self):
        self.assertEqual(_band("0,1"), (0.0, 1.0))
        for text in ("0.8,0.2", "0.5", "a,b", "0,1.5"):
            with self.assertRaises(argparse.ArgumentTypeError):
                _band(text)

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

    def test_unobservable_spend_is_the_bench_s_fault_only_on_the_native_reference(self):
        row = self._row(spend_stop="spend could not be observed for 30s: no usage source", spend_unobserved=True)
        self.assertIn("bench", infra_error(row, "native"))
        self.assertIsNone(infra_error(row, "new"))
        self.assertIn("spend guard", harness_failure(row, "new"))
        native = record(self.SPEC, {**self.ARM, "key": "haiku/native", "harness": "native"}, 0, row, self.GRADE,
                        None, None)
        self.assertEqual((native["passed"], native["harness_failure"]), (False, None))

    def test_a_spend_stop_is_a_harness_failure_on_every_arm(self):
        row = self._row(spend_stop="spend $0.51 reached the trial allowance $0.50")
        self.assertIsNone(infra_error(row, "native"))
        self.assertIn("spend guard", harness_failure(row, "native"))
        self.assertFalse(record(self.SPEC, self.ARM, 0, row, self.GRADE, 0.52, 9)["passed"])


if __name__ == "__main__":
    unittest.main()
