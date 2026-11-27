"""The shared release run: Archon old/new arms, parity, the failure digest, the per-product verdicts and the
parallel scheduler. No test calls a model."""
import contextlib
import io
import tempfile
import threading
import time
import unittest
import unittest.mock
from collections import Counter
from pathlib import Path

import release_bench
import release_bench_trial
from release_bench_host import default_jobs, default_load_max, read_invocations, record_invocation
from release_bench_outcomes import (
    DIGEST_CATEGORIES,
    digest_document,
    failure_category,
    failure_digest,
    parity,
    parity_line,
)
from release_bench_arms import ARCHON_DEFAULTS, build_arm
from release_bench_calibrate import SELECTION_DEFAULTS, release_minutes, task_rates
from release_bench_pool import Gates, run_pool, schedule
from release_bench_report import archon_verdict, combined_verdict, render

MODELS = ("haiku", "luna")


def rec(task, arm, passed, repeat=0, **extra):
    model, harness = arm.split("/")
    item = {"task": task, "arm": arm, "model": model, "harness": harness, "repeat": repeat, "passed": passed,
            "fraction": 1.0 if passed else 0.5, "cost_usd": 0.01, "infra_error": None, "harness_failure": None,
            "template": "t", "family": "f", "checks_passed": 2 if passed else 1, "checks_total": 2,
            "seconds": 10, "workflow": "completed", "workflow_error": None, "grade_error": None}
    item.update(extra)
    return item


def matrix(outcomes, tasks=20):
    """Records of `tasks` tasks on each arm; `outcomes[arm](index)` says whether task `index` passed."""
    return [rec(f"task-{index}", arm, outcome(index)) for arm, outcome in outcomes.items() for index in range(tasks)]


def plan(arms, tasks=20):
    return {"arms": list(arms), "tasks": [f"task-{index}" for index in range(tasks)], "repeats": 1}


class ParityTests(unittest.TestCase):
    def test_archon_at_or_above_plain_aether_meets_parity(self):
        records = matrix({"haiku/new": lambda i: i % 2 == 0, "haiku/archon-new": lambda i: i % 2 == 0 or i == 1})
        result = parity(records, "haiku")
        self.assertEqual((result["status"], result["behind"]), ("MET", []))
        self.assertIn("PARITY MET", parity_line(result))

    def test_a_miss_names_the_tasks_new_passed_more_often(self):
        records = matrix({"haiku/new": lambda i: i < 10, "haiku/archon-new": lambda i: i < 8})
        result = parity(records, "haiku")
        self.assertEqual((result["status"], result["behind"]), ("MISSED", ["task-8", "task-9"]))
        line = parity_line(result)
        self.assertIn("PARITY MISSED", line)
        self.assertIn("task-8, task-9", line)

    def test_parity_compares_only_cells_both_arms_counted(self):
        records = matrix({"haiku/new": lambda i: i < 10, "haiku/archon-new": lambda i: i < 10})
        # An outage on archon-new's passing task is not a loss: that cell is left out on both sides.
        records = [dict(item, infra_error="provider: 529") if item["arm"] == "haiku/archon-new"
                   and item["task"] == "task-0" else item for item in records]
        self.assertEqual(parity(records, "haiku")["status"], "MET")
        self.assertEqual(parity(records, "haiku")["pairs"], 19)
        self.assertEqual(parity([], "luna")["status"], "UNKNOWN")


class DigestTests(unittest.TestCase):
    def test_every_way_a_trial_ends_has_one_category(self):
        cases = {
            DIGEST_CATEGORIES[0]: {"infra_error": "provider: 529"},
            DIGEST_CATEGORIES[1]: {"harness_failure": "spend guard: allowance reached"},
            DIGEST_CATEGORIES[2]: {"workflow": "timeout"},
            DIGEST_CATEGORIES[3]: {"workflow": "failed", "workflow_error": "review round cap reached; stopping"},
            DIGEST_CATEGORIES[4]: {"workflow": "failed", "workflow_error": "stage build: ESCALATE: too risky here"},
            DIGEST_CATEGORIES[5]: {"harness_failure": "no model output: exit 1", "workflow": "failed"},
            DIGEST_CATEGORIES[6]: {"workflow": "failed", "workflow_error": "item failed"},
            DIGEST_CATEGORIES[7]: {"grade_error": "grading exceeded 120s", "checks_passed": 0},
            DIGEST_CATEGORIES[8]: {"checks_passed": 0},
            DIGEST_CATEGORIES[9]: {"checks_passed": 1, "checks_total": 3},
            DIGEST_CATEGORIES[10]: {"checks_passed": 2, "checks_total": 3},
        }
        self.assertEqual(set(cases), set(DIGEST_CATEGORIES))
        for category, extra in cases.items():
            with self.subTest(category=category):
                self.assertEqual(failure_category(rec("x", "haiku/new", False, **extra)), category)
        self.assertIsNone(failure_category(rec("x", "haiku/new", True)))

    def test_the_digest_counts_per_arm_and_names_no_task(self):
        records = [rec("secret-task-1", "haiku/new", True), rec("secret-task-2", "haiku/new", False),
                   rec("secret-task-3", "haiku/archon-new", False, workflow="timeout")]
        digest = failure_digest(records, ["haiku/new", "haiku/archon-new", "luna/new"])
        self.assertEqual(digest["haiku/new"]["passed"], 1)
        self.assertEqual(digest["haiku/archon-new"]["categories"], Counter({"timeout": 1}))
        self.assertEqual(digest["luna/new"]["trials"], 0)
        document = digest_document(digest, ["- Arms: haiku/new"])
        self.assertNotIn("secret", document)
        self.assertNotIn("family", document.lower().replace("by category", ""))
        self.assertIn("timeout 1", document)


ALL_HAIKU = ("haiku/native", "haiku/old", "haiku/new", "haiku/archon-old", "haiku/archon-new")


class ProductVerdictTests(unittest.TestCase):
    def _render(self, outcomes, arms=ALL_HAIKU):
        records = matrix(outcomes)
        return render(records, ["- header"], MODELS, {}, plan(arms))

    def test_one_run_judges_both_products(self):
        same = lambda i: i % 2 == 0  # noqa: E731
        report = self._render({arm: same for arm in ALL_HAIKU})
        self.assertEqual(report["products"], {"aether": "PASS", "archon": "PASS"})
        self.assertEqual(report["overall"], "PASS")
        for section in ("Aether release gate", "Archon release gate", "PARITY MET", "Failure digest",
                        "Per model", "Per task: haiku", "Load and wall time"):
            self.assertIn(section, report["text"])

    def test_a_parity_miss_fails_archon_but_not_aether(self):
        outcomes = {arm: (lambda i: i < 12) for arm in ALL_HAIKU}
        outcomes["haiku/archon-old"] = outcomes["haiku/archon-new"] = lambda i: i < 11
        report = self._render(outcomes)
        self.assertEqual(report["products"], {"aether": "PASS", "archon": "FAIL"})
        self.assertEqual(report["overall"], "FAIL")
        self.assertIn("PARITY MISSED", report["text"])

    def test_an_archon_regression_fails_the_archon_gate(self):
        outcomes = {arm: (lambda i: True) for arm in ALL_HAIKU}
        outcomes["haiku/archon-new"] = lambda i: i < 10
        report = self._render(outcomes)
        self.assertEqual(report["archon_verdicts"]["haiku"]["verdict"], "FAIL")
        self.assertEqual(report["products"]["archon"], "FAIL")

    def test_without_archon_arms_only_aether_is_judged(self):
        arms = ("haiku/native", "haiku/old", "haiku/new")
        report = self._render({arm: (lambda i: True) for arm in arms}, arms)
        self.assertEqual(report["products"], {"aether": "PASS", "archon": None})
        self.assertNotIn("Archon release gate", report["text"])

    def test_parity_alone_cannot_pass_an_archon_release(self):
        gates, met = {}, {"haiku": {"status": "MET"}}
        self.assertEqual(archon_verdict(gates, met), "INCONCLUSIVE")
        self.assertEqual(archon_verdict(gates, {"haiku": {"status": "MISSED"}}), "FAIL")
        self.assertIsNone(archon_verdict({}, {}))
        self.assertEqual(combined_verdict({"aether": "PASS", "archon": None}), "PASS")
        self.assertEqual(combined_verdict({"aether": "PASS", "archon": "INCONCLUSIVE"}), "INCONCLUSIVE")
        self.assertEqual(combined_verdict({"aether": None, "archon": None}), "INCONCLUSIVE")


class SchedulerTests(unittest.TestCase):
    ARMS = ["haiku/native", "haiku/old", "haiku/new", "haiku/archon-old", "haiku/archon-new",
            "luna/native", "luna/old", "luna/new", "luna/archon-old", "luna/archon-new"]

    def test_every_compared_pair_alternates_which_side_starts(self):
        order = schedule([f"t{index}" for index in range(10)], self.ARMS, 2, seed=3)
        first = Counter()
        for start in range(0, len(order), len(self.ARMS)):
            block = [arm for _, _, arm in order[start:start + len(self.ARMS)]]
            self.assertEqual(len({(task, repeat) for task, repeat, _ in order[start:start + len(self.ARMS)]}), 1)
            for model in MODELS:
                for old, new in (("old", "new"), ("archon-old", "archon-new")):
                    first[(model, old)] += block.index(f"{model}/{old}") < block.index(f"{model}/{new}")
        self.assertEqual(set(first.values()), {10})

    def test_trials_of_different_tasks_and_arms_run_concurrently_under_the_cap(self):
        running, peak, lock = [0], [0], threading.Lock()

        def run_one(triple):
            with lock:
                running[0] += 1
                peak[0] = max(peak[0], running[0])
            time.sleep(0.05)
            with lock:
                running[0] -= 1
            return rec(triple[0], triple[2], True, triple[1])
        order = schedule([f"t{index}" for index in range(4)], ["haiku/new", "haiku/archon-new"], 1, seed=0)
        with tempfile.TemporaryDirectory() as directory:
            records, stopped = run_pool(order, run_one, lambda _triple: 0.1, jobs=3,
                                        gates=Gates(load_max=10_000, mem_min_gb=0, budget_usd=10),
                                        results_path=str(Path(directory, "results.jsonl")),
                                        attempts_path=str(Path(directory, "attempts.jsonl")), log=lambda _line: None)
        self.assertIsNone(stopped)
        self.assertEqual((len(records), peak[0]), (8, 3))
        self.assertTrue(all(item["load_at_start"] is not None for item in records))

    def test_host_defaults_scale_with_cores_and_memory(self):
        self.assertEqual(default_jobs(cpus=12, memory_gb=15), 10)
        self.assertEqual(default_jobs(cpus=8, memory_gb=32), 6)
        self.assertEqual(default_jobs(cpus=16, memory_gb=6), 5)
        self.assertEqual(default_jobs(cpus=2, memory_gb=2), 2)
        self.assertEqual(default_load_max(cpus=12), 15.0)
        self.assertEqual(default_load_max(cpus=8), 10.0)

    def test_invocations_record_their_wall_time(self):
        with tempfile.TemporaryDirectory() as directory:
            record_invocation(directory, time.monotonic() - 60, 4, 10)
            invocations = record_invocation(directory, time.monotonic() - 30, 4, 5)
            self.assertEqual(read_invocations(directory), invocations)
        self.assertEqual([item["trials_run"] for item in invocations], [10, 5])
        self.assertGreaterEqual(invocations[0]["wall_seconds"], 60)

    def test_the_header_reports_the_speed_up_over_serial(self):
        records = [rec(f"t{index}", "haiku/new", True) for index in range(6)]
        lines = release_bench._speed_lines(records, [{"wall_seconds": 20, "trials_run": 6, "jobs": 3}])
        self.assertIn("3.00x", lines[0])
        self.assertEqual(release_bench._speed_lines(records, [{"wall_seconds": 20, "trials_run": 5, "jobs": 3}]), [])


class ArchonSideTests(unittest.TestCase):
    def test_a_reviewed_archon_arm_may_start_its_reviewer_and_reserves_for_it(self):
        reviewed = build_arm("haiku", "archon-new", archon_options={**ARCHON_DEFAULTS, "review": "standard"})
        ablation = build_arm("haiku", "archon-new", archon_options={**ARCHON_DEFAULTS, "review": "none"})
        self.assertNotIn("Do not delegate", reviewed["condition"]["suffix"])
        self.assertIn("reviewer", reviewed["condition"]["suffix"])
        self.assertIn("Do not delegate", ablation["condition"]["suffix"])
        self.assertEqual((reviewed["max_agents"], ablation["max_agents"]), (3, 2))
        self.assertEqual(reviewed["condition"]["max_concurrent_turns"], 3)

    def test_each_archon_side_runs_its_own_archon_and_aether(self):
        seen = []
        context = {"timeout": 5, "binaries": {"old": "/aether-old", "new": "/aether-new"},
                   "archon_bins": {"old": "/archon-old", "new": "/archon-new"},
                   "price_manifest": {"price_table": {}}}

        def fake(archon_bin, aether_binary, *_args):
            seen.append((archon_bin, aether_binary))
        with tempfile.TemporaryDirectory() as directory, \
                unittest.mock.patch.object(release_bench_trial, "run_archon_trial", fake), \
                unittest.mock.patch.object(release_bench_trial, "_guard") as guard, \
                unittest.mock.patch.object(release_bench_trial, "price_of", return_value=None):
            guard.return_value.__enter__.return_value = unittest.mock.Mock(allowance_usd=1, peak_usd=0, tripped=None)
            for side in ("old", "new"):
                arm = release_bench.build_arm("haiku", f"archon-{side}")
                release_bench_trial._solve_in(arm, "task", directory, directory, context, {}, "m", {})
        self.assertEqual(seen, [("/archon-old", "/aether-old"), ("/archon-new", "/aether-new")])

    def test_archon_bin_is_an_alias_of_archon_new(self):
        base = ["run", "--old", "a", "--new", "b"]
        args = release_bench.parse_args(base + ["--archon-bin", "x"])
        self.assertEqual((str(args.archon_new), release_bench.archon_sides(args)), ("x", ("new",)))
        args = release_bench.parse_args(base + ["--archon-old", "o", "--archon-new", "n"])
        self.assertEqual(release_bench.archon_sides(args), ("old", "new"))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            release_bench.parse_args(base + ["--archon-bin", "x", "--archon-new", "y"])



class CandidateCalibrationTests(unittest.TestCase):
    def _records(self):
        return [rec("t-0", "haiku/native", True), rec("t-0", "haiku/new", False, seconds=20),
                rec("t-0", "haiku/new", True, 1, seconds=40), rec("t-0", "luna/new", True, seconds=60),
                rec("t-0", "haiku/archon-new", True, seconds=90), rec("t-0", "luna/native", False)]

    def test_rates_follow_the_selected_harness_and_keep_each_harness_s_time(self):
        row = task_rates(self._records(), "new")["t-0"]
        self.assertEqual((row["haiku"], row["haiku_n"], row["luna"], row["luna_n"]), (0.5, 2, 1.0, 1))
        self.assertEqual((row["haiku_new_seconds"], row["haiku_archon-new_seconds"]), (30, 90))
        self.assertEqual(row["haiku_seconds"], 10, "the Aether-only time model counts native seconds")
        self.assertEqual(task_rates(self._records())["t-0"]["haiku"], 1.0)

    def test_the_shared_release_time_counts_ten_arms_and_guesses_missing_archon_time(self):
        row = task_rates(self._records(), "new")["t-0"]
        settings = {**SELECTION_DEFAULTS, "release_arms": "shared"}
        haiku = 10 + 2 * 30 + 2 * 90
        luna = 10 + 2 * 60 + 2 * 60 * 2.0
        self.assertAlmostEqual(release_minutes({"t-0": row}, ["t-0"], 1, settings), (haiku + luna) / 60)

    def test_calibration_takes_candidate_arms_and_refuses_old_ones(self):
        base = ["calibrate", "--price-binary", "/x", "--select-harness", "new"]
        args = release_bench.parse_args([*base, "--arms", "haiku/new,luna/new,haiku/archon-new", "--archon-new", "/a",
                                         "--luna-band", "0.3,0.7"])
        self.assertEqual(args.arm_keys, ["haiku/new", "luna/new", "haiku/archon-new"])
        settings = release_bench.selection_settings(args)
        self.assertEqual((settings["select_harness"], settings["luna_band"]), ("new", (0.3, 0.7)))
        for arms in ("haiku/old", "haiku/archon-new", "haiku/native"):
            with self.subTest(arms=arms), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                release_bench.parse_args([*base, "--arms", arms])

    def test_a_frozen_set_s_trial_timeout_applies_unless_one_is_asked_for(self):
        self.assertEqual(release_bench.trial_timeout(None, {"trial_timeout_seconds": 1200}), 1200)
        self.assertEqual(release_bench.trial_timeout(700, {"trial_timeout_seconds": 1200}), 700)
        self.assertEqual(release_bench.trial_timeout(None, {}), release_bench.TRIAL_TIMEOUT_SECONDS)
        with self.assertRaises(SystemExit):
            release_bench.trial_timeout(5)

    def test_a_resumed_calibration_may_reselect_but_not_rerun_differently(self):
        with tempfile.TemporaryDirectory() as directory:
            run_dir = Path(directory)
            release_bench._config_check(run_dir, {"arms": ["haiku/new"], "selection": {"min_size": 20}})
            release_bench._config_check(run_dir, {"arms": ["haiku/new"], "selection": {"min_size": 22}},
                                        reselect=("selection",))
            self.assertIn('"min_size": 22', (run_dir / "config.json").read_text())
            with self.assertRaises(SystemExit):
                release_bench._config_check(run_dir, {"arms": ["luna/new"], "selection": {"min_size": 22}},
                                            reselect=("selection",))

    def test_a_recorded_selection_is_restored_with_its_bands_as_tuples(self):
        settings = release_bench.recorded_selection({"selection": {"luna_band": [0.3, 0.7], "select_harness": "new"}})
        self.assertEqual((settings["luna_band"], settings["haiku_band"]), ((0.3, 0.7), SELECTION_DEFAULTS["haiku_band"]))
        self.assertEqual(release_bench.recorded_selection({"haiku_band": [0.1, 0.9]})["haiku_band"], (0.1, 0.9))


if __name__ == "__main__":
    unittest.main()
