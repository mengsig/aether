"""Release bench teardown: a cleanup error after grading is a flag, the stop is retried, ungraded stays infra."""
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import harness_bench as hb
import release_bench_archon
import release_bench_trial
from release_bench_outcomes import digest_lines, failure_category, failure_digest
from release_bench_report import _arm_table, arm_summary
from release_bench_trial import record, run_release_trial

SPEC = {"id": "tiny-0", "family": "f", "template": "tiny", "checks": 4, "prompt": "fix it",
        "files": {"a.py": "x = 1\n"}, "history": [], "tags": {}, "meta": {}, "seed": 0,
        "hidden": "import unittest\n", "solution": {"a.py": "x = 2\n"}}
ARMS = {harness: {"key": f"haiku/{harness}", "model": "haiku", "harness": harness,
                  "condition": {"kind": "test", "suffix": ""}}
        for harness in ("native", "new", "archon-new")}
PASS = {"passed": 4, "total": 4, "failed": [], "error": None}
UNGRADED = {"passed": 0, "total": 4, "failed": ["*"], "error": "grader exited 1 without a report: boom"}
STOP_TIMEOUT = "Command '['aether', 'stop']' timed out after 10 seconds"


def row(**extra):
    return {"trial_dir": "/x", "started_at": "2026-01-01T00:00:00Z", "workflow_status": "completed",
            "usage_report": {"usage": {"output_tokens": 10}}, **extra}


class CleanupRecordTests(unittest.TestCase):
    def test_a_teardown_error_after_a_passing_grade_is_only_a_flag(self):
        cases = (("new", "daemon_cleanup_error"), ("archon-new", "cleanup_error"), ("native", "cleanup_error"),
                 ("new", "tmpdir_cleanup_error"))
        for harness, key in cases:
            with self.subTest(harness=harness, key=key):
                item = record(SPEC, ARMS[harness], 0, row(**{key: STOP_TIMEOUT}), PASS, 0.01, 10)
                self.assertTrue(item["passed"], "a teardown error never flips a passing grade")
                self.assertEqual((item["infra_error"], item["harness_failure"]), (None, None))
                self.assertIn(key, item["harness_cleanup"])
                self.assertIn("timed out", item["harness_cleanup"])

    def test_a_teardown_error_after_a_failing_grade_keeps_the_trial_counted(self):
        item = record(SPEC, ARMS["new"], 0, row(daemon_cleanup_error=STOP_TIMEOUT), {**PASS, "passed": 1}, 0.01, 10)
        self.assertFalse(item["passed"])
        self.assertIsNone(item["infra_error"], "a graded trial is counted whatever its teardown did")
        self.assertIsNotNone(item["harness_cleanup"])
        self.assertEqual(failure_category(item), "wrong result: under half the hidden checks pass")

    def test_a_teardown_error_that_kept_the_workspace_from_grading_is_infra(self):
        for harness in ARMS:
            with self.subTest(harness=harness):
                item = record(SPEC, ARMS[harness], 0, row(cleanup_error="socket left"), UNGRADED, None, None)
                self.assertFalse(item["passed"])
                self.assertIn("cleanup prevented grading", item["infra_error"])
                self.assertIn("socket left", item["infra_error"])
                self.assertIsNone(item["harness_failure"])

    def test_an_ungraded_trial_with_a_clean_teardown_is_not_infra(self):
        item = record(SPEC, ARMS["new"], 0, row(), UNGRADED, 0.01, 10)
        self.assertEqual((item["infra_error"], item["harness_cleanup"], item["passed"]), (None, None, False))
        self.assertEqual(failure_category(item), "result not gradable")

    def test_a_telemetry_error_stays_a_harness_failure(self):
        item = record(SPEC, ARMS["new"], 0, row(telemetry_error="KeyError: x"), PASS, 0.01, 10)
        self.assertFalse(item["passed"])
        self.assertIn("telemetry_error", item["harness_failure"])
        self.assertIsNone(item["harness_cleanup"])


class CleanupReportTests(unittest.TestCase):
    def _records(self):
        flagged = record(SPEC, ARMS["new"], 0, row(daemon_cleanup_error=STOP_TIMEOUT), PASS, 0.01, 10)
        clean = record({**SPEC, "id": "tiny-1"}, ARMS["new"], 0, row(), PASS, 0.01, 10)
        dropped = record({**SPEC, "id": "tiny-2"}, ARMS["new"], 0, row(cleanup_error="x"), UNGRADED, None, None)
        return [flagged, clean, dropped]

    def test_the_digest_counts_flags_beside_the_verdicts(self):
        digest = failure_digest(self._records(), ["haiku/new"])
        self.assertEqual((digest["haiku/new"]["passed"], digest["haiku/new"]["cleanup_flags"]), (2, 1),
                         "a trial dropped as infra is not also a cleanup flag")
        lines = digest_lines(digest)
        self.assertIn("Teardown failed after grading", lines[0])
        self.assertTrue(lines[-1].startswith("| haiku/new | 3 | 2 | 1 |"), lines[-1])

    def test_the_report_shows_the_flags_per_arm(self):
        summary = arm_summary(self._records(), "haiku/new")
        self.assertEqual((summary["cleanup_flags"], summary["infra"], summary["harness_failures"]), (1, 1, 0))
        table = _arm_table([summary], 3)
        self.assertIn("Cleanup flags", table[0])
        self.assertEqual(table[0].count("|"), table[1].count("|"), "the header and its rule have equal columns")
        self.assertIn("| 1 | 0 | 1 |", table[2])


class GradeAfterTeardownTests(unittest.TestCase):
    def _run(self, root, solve):
        context = {"run_dir": str(Path(root, "run")), "work_dir": str(Path(root, "work")),
                   "price_manifest": {"price_table": {"models": []}}}
        grades = []

        def fake_grade(workspace, *args):
            grades.append(Path(workspace))
            return PASS
        with unittest.mock.patch.object(release_bench_trial, "_solve", solve), \
                unittest.mock.patch.object(release_bench_trial, "grade", fake_grade):
            item = run_release_trial(SPEC, ARMS["new"], 0, context)
        return item, grades

    def test_a_trial_whose_stop_timed_out_is_graded_and_passes(self):
        def solve(arm, task_text, workspace, trial_dir, context, result, marker):
            result.update(workflow_status="completed", usage_report={"usage": {"output_tokens": 5}},
                          daemon_cleanup_error=STOP_TIMEOUT)
        with tempfile.TemporaryDirectory() as root:
            item, grades = self._run(root, solve)
        self.assertEqual(len(grades), 1)
        self.assertTrue(item["passed"])
        self.assertIn("daemon_cleanup_error", item["harness_cleanup"])

    def test_a_bench_crash_in_teardown_still_grades_but_stays_infra(self):
        def solve(*args):
            raise OSError("socket directory vanished")
        with tempfile.TemporaryDirectory() as root:
            item, grades = self._run(root, solve)
        self.assertEqual(len(grades), 1, "the workspace is graded whatever teardown did")
        self.assertFalse(item["passed"])
        self.assertIn("bench: OSError", item["infra_error"])


class StopRetryTests(unittest.TestCase):
    def _stop(self, outcomes):
        """stop_daemon with `aether stop` returning or raising each of `outcomes`; (error, timeouts used)."""
        timeouts = []
        with tempfile.TemporaryDirectory() as root:
            socket = Path(root, "aether.sock")
            socket.write_text("")

            def fake_run(command, timeout, **kwargs):
                timeouts.append(timeout)
                outcome = outcomes[len(timeouts) - 1]
                if isinstance(outcome, BaseException):
                    raise outcome
                if outcome == 0:
                    socket.unlink()
                return subprocess.CompletedProcess(command, outcome, "", "stop failed")
            with unittest.mock.patch.object(hb.subprocess, "run", fake_run):
                error = hb.stop_daemon("aether", {"AETHER_SOCKET": str(socket)}, None)
        return error, timeouts

    def test_a_timed_out_stop_is_retried_with_a_longer_timeout(self):
        error, timeouts = self._stop([subprocess.TimeoutExpired(["aether", "stop"], 10), 0])
        self.assertIsNone(error, "a stop that succeeds on its retry leaves no cleanup error")
        self.assertEqual(timeouts, list(hb.STOP_TIMEOUTS_SECONDS))
        self.assertGreater(timeouts[1], timeouts[0])

    def test_a_stop_that_fails_twice_is_flagged_with_both_errors(self):
        error, timeouts = self._stop([subprocess.TimeoutExpired(["aether", "stop"], 10), 1])
        self.assertEqual(len(timeouts), 2, "the stop is retried exactly once")
        self.assertIn("timed out", error)
        self.assertIn("retry after 30s: daemon stop exited 1", error)

    def test_a_clean_stop_is_not_retried(self):
        error, timeouts = self._stop([0])
        self.assertEqual((error, timeouts), (None, [10]))

    def test_the_archon_stop_is_retried_with_a_longer_timeout(self):
        timeouts = []

        def fake_archon(archon_bin, env, args, log, timeout=release_bench_archon.COMMAND_SECONDS):
            timeouts.append(timeout)
            raise subprocess.TimeoutExpired(args, timeout)
        with tempfile.TemporaryDirectory() as root, \
                unittest.mock.patch.object(release_bench_archon, "archon", fake_archon):
            error = release_bench_archon._stop("archon", {}, [], {"socket_dir": None, "root": root})
        self.assertEqual(timeouts, [release_bench_archon.COMMAND_SECONDS, release_bench_archon.STOP_RETRY_SECONDS])
        self.assertGreater(timeouts[1], timeouts[0])
        self.assertIn(f"retry after {release_bench_archon.STOP_RETRY_SECONDS}s", error)


if __name__ == "__main__":
    unittest.main()
