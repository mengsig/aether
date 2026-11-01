"""The history analysis: conditional logit, permutation trend, McNemar tails and the CSV round trip."""
import itertools
import math
import random
import unittest

from release_bench_history import (
    _sum_counts,
    analyze_tier,
    complete_blocks,
    conditional_logit,
    export_rows,
    missing_cells,
    one_sided_decline_p,
    permutation_trend,
    records_of,
    version_key,
)

VERSIONS = ["v1.0.0", "v1.0.5", "v1.0.10", "v1.1.1"]
STEPS = {version: index for index, version in enumerate(VERSIONS)}


def rows_from(pass_of, tasks=40, repeats=1):
    """CSV rows in which `pass_of(task index, release index, repeat)` decides each trial."""
    rows = []
    for task, (index, version), repeat in itertools.product(range(tasks), enumerate(VERSIONS), range(repeats)):
        passed = int(pass_of(task, index, repeat))
        rows.append({"tier": "luna", "version": version, "task": f"t-{task}", "family": "f", "repeat": str(repeat),
                     "passed": str(passed), "graded_pass": str(passed), "checks_passed": str(2 if passed else 1),
                     "checks_total": "2", "cost_usd": "0.01", "seconds": "30", "harness_failure": "",
                     "infra_error": ""})
    return rows


def chance(rate_of):
    """A seeded pass decision that passes release `index` with probability `rate_of(index)`."""
    return lambda task, index, repeat: random.Random(task * 1000 + index * 10 + repeat).random() < rate_of(index)


def brute_loglik(beta, strata):
    """The conditional log-likelihood summed over every subset explicitly."""
    total = 0.0
    for points in strata.values():
        k = sum(passed for _, passed in points)
        xs = [x for x, _ in points]
        observed = sum(x for x, passed in points if passed)
        norm = sum(math.exp(beta * sum(subset)) for subset in itertools.combinations(xs, k))
        total += beta * observed - math.log(norm)
    return total


class HistoryStatsTests(unittest.TestCase):
    def test_tags_sort_numerically(self):
        self.assertEqual(sorted(["v1.0.10", "v1.0.5", "v1.2.0"], key=version_key), ["v1.0.5", "v1.0.10", "v1.2.0"])
        with self.assertRaises(AssertionError):
            version_key("1.0.0")

    def test_subset_sums_match_enumeration(self):
        points = [(0, 1), (1, 0), (2, 1), (2, 0), (3, 1)]
        expected = {}
        for subset in itertools.combinations([x for x, _ in points], 3):
            expected[sum(subset)] = expected.get(sum(subset), 0) + 1
        self.assertEqual(dict(_sum_counts(points)), expected)

    def test_the_conditional_logit_maximizes_the_enumerated_likelihood(self):
        rng = random.Random(3)
        strata = {task: [(x, int(rng.random() < 0.7 - 0.1 * x)) for x in range(4) for _ in range(2)]
                  for task in range(30)}
        beta, se, z, _ = conditional_logit(strata)
        self.assertLess(beta, 0)
        for other in (beta - 0.05, beta + 0.05):
            self.assertGreater(brute_loglik(beta, strata), brute_loglik(other, strata))
        deviance = 2 * (brute_loglik(beta, strata) - brute_loglik(0.0, strata))
        self.assertAlmostEqual(z, -math.sqrt(deviance), places=6)
        self.assertGreater(se, 0)

    def test_a_steady_decline_is_found_and_its_mirror_is_not(self):
        decline = analyze_tier(rows_from(chance(lambda index: 0.8 - 0.2 * index)), "luna")
        self.assertTrue(decline["declines"]["trend"])
        self.assertTrue(decline["declines"]["newest_below_oldest"])
        self.assertLess(decline["trend"]["slope"], 0)
        rise = analyze_tier(rows_from(chance(lambda index: 0.2 + 0.2 * index)), "luna")
        self.assertFalse(rise["declines"]["trend"])
        self.assertFalse(rise["declines"]["newest_below_oldest"])
        self.assertGreater(rise["trend"]["beta"], 0)

    def test_spacing_by_every_release_rescales_the_slope_but_keeps_its_sign(self):
        rows = rows_from(chance(lambda index: 0.8 - 0.2 * index))
        doubled = analyze_tier(rows, "luna", positions={version: 2 * index for version, index in STEPS.items()})
        self.assertAlmostEqual(doubled["spaced_trend"]["beta"], doubled["trend"]["beta"] / 2, places=6)
        self.assertAlmostEqual(doubled["spaced_trend"]["logit_decline_p"], doubled["trend"]["logit_decline_p"])
        with self.assertRaises(ValueError):
            analyze_tier(rows, "luna", positions={"v1.0.0": 0})
        with self.assertRaises(ValueError):
            analyze_tier(rows, "luna", positions={**STEPS, "v1.2.0": 9})

    def test_a_version_short_of_cells_is_refused_not_dropped(self):
        rows = rows_from(chance(lambda index: 0.6))
        short = [row for row in rows if not (row["version"] == "v1.0.5" and row["task"] == "t-3")]
        with self.assertRaises(ValueError):
            analyze_tier(short, "luna")
        result = analyze_tier(rows, "luna")
        self.assertEqual(result["versions"], VERSIONS)
        self.assertEqual(set(result["declines"]), {"trend", "trend_permutation", "newest_below_oldest"})

    def test_the_permutation_test_shuffles_within_each_task_and_repeat(self):
        # Every task passes only on its first release: the steepest decline the data allows.
        records = records_of(rows_from(lambda task, index, repeat: index == 0 and (task + repeat) % 2 == 0,
                                       repeats=2))
        slope, (low, high), decline_p, two_sided_p = permutation_trend(records, STEPS, permutations=2000)
        self.assertLess(slope, 0)
        self.assertLessEqual(low, slope)
        self.assertLessEqual(slope, high)
        self.assertLess(decline_p, 0.01)
        self.assertLessEqual(decline_p, two_sided_p)

    def test_the_one_sided_mcnemar_tail(self):
        self.assertAlmostEqual(one_sided_decline_p(5, 0), 1 / 32)
        self.assertAlmostEqual(one_sided_decline_p(0, 5), 1.0)
        self.assertEqual(one_sided_decline_p(0, 0), 1.0)
        self.assertAlmostEqual(one_sided_decline_p(3, 3), 0.65625)

    def test_export_names_the_old_and_new_arms_by_tag(self):
        base = {"model": "haiku", "task": "t-0", "family": "f", "repeat": 0, "checks_passed": 2, "checks_total": 2,
                "cost_usd": 0.01, "seconds": 3.0, "infra_error": None}
        records = [{**base, "harness": "new", "passed": False, "harness_failure": "daemon_cleanup_error: x"},
                   {**base, "harness": "old", "passed": True, "harness_failure": None},
                   {**base, "harness": "v1.0.0", "passed": True, "harness_failure": None}]
        rows = export_rows(records, "v1.2.7", "v1.2.8")
        self.assertEqual([row["version"] for row in rows], ["v1.0.0", "v1.2.7", "v1.2.8"])
        self.assertEqual((rows[2]["passed"], rows[2]["graded_pass"]), (0, 1))
        with self.assertRaises(AssertionError):
            export_rows(records + [records[0]], "v1.2.7", "v1.2.8")

    def test_missing_cells_name_every_planned_trial_without_a_record(self):
        config = {"arms": ["haiku/old", "haiku/v1.0.0"], "set": {"task_ids": ["t-0", "t-1"]}, "repeats": 2}
        records = [{"arm": arm, "task": task, "repeat": repeat} for arm in config["arms"]
                   for task in ("t-0", "t-1") for repeat in (0, 1)]
        self.assertEqual(missing_cells(records, config), [])
        self.assertEqual(missing_cells(records[1:], config), [("haiku/old", "t-0", 0)])
        no_v100 = [item for item in records if item["arm"] != "haiku/v1.0.0"]
        self.assertEqual(len(missing_cells(no_v100, config)), 4)
        with self.assertRaises(AssertionError):
            missing_cells(records + [{"arm": "haiku/new", "task": "t-0", "repeat": 0}], config)

    def test_a_partial_export_keeps_only_blocks_every_arm_ran(self):
        config = {"arms": ["haiku/old", "haiku/v1.0.0"], "set": {"task_ids": ["t-0", "t-1"]}, "repeats": 1}
        records = [{"arm": arm, "task": task, "repeat": 0} for arm in config["arms"] for task in ("t-0", "t-1")]
        unfinished = records[:3]
        kept = complete_blocks(unfinished, missing_cells(unfinished, config))
        self.assertEqual({(item["task"], item["arm"]) for item in kept},
                         {("t-0", "haiku/old"), ("t-0", "haiku/v1.0.0")})
        self.assertEqual(complete_blocks(records, []), records)


if __name__ == "__main__":
    unittest.main()
