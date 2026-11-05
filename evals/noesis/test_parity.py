"""Tests of the parity manifest checker. They read files only: no cargo, no model."""
import copy
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import parity


def case(cid, **extra):
    base = {"id": cid, "behaviour": "b", "noesis_form": "same", "tests": []}
    return dict(base, **extra)


class CheckerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        src = self.repo / "crates" / "aether-noesis" / "src"
        src.mkdir(parents=True)
        (src / "a.rs").write_text("#[test]\nfn present() {}\nasync fn also_present() {}\n")

    def entry(self, name):
        return {"crate": "aether-noesis", "file": "crates/aether-noesis/src/a.rs", "name": name}

    def check(self, cases, required):
        return parity.check_cases(cases, self.repo, required)

    def test_a_sound_manifest_passes(self):
        cases = [case("T1", tests=[self.entry("present"), self.entry("also_present")])]
        self.assertEqual(self.check(cases, ["T1"]), [])

    def test_a_missing_test_name_is_reported(self):
        problems = self.check([case("T1", tests=[self.entry("gone")])], ["T1"])
        self.assertEqual(len(problems), 1)
        self.assertIn("no fn gone", problems[0])

    def test_a_missing_file_is_reported(self):
        entry = dict(self.entry("present"), file="crates/aether-noesis/src/none.rs")
        problems = self.check([case("T1", tests=[entry])], ["T1"])
        self.assertIn("does not exist", problems[0])

    def test_a_duplicate_id_is_reported(self):
        cases = [case("T1", gap="why"), case("T1", gap="why")]
        self.assertTrue(any("duplicate id" in p for p in self.check(cases, ["T1"])))

    def test_a_missing_and_an_unknown_id_are_reported(self):
        problems = self.check([case("Z9", gap="why")], ["T1"])
        self.assertTrue(any("T1: missing" in p for p in problems))
        self.assertTrue(any("Z9: not a known" in p for p in problems))

    def test_an_empty_row_needs_a_gap(self):
        self.assertTrue(any("no tests and no gap" in p for p in self.check([case("T1")], ["T1"])))
        self.assertEqual(self.check([case("T1", gap="no test covers it")], ["T1"]), [])

    def test_a_bad_noesis_form_is_reported(self):
        bad = case("T1", gap="g", noesis_form="different")
        self.assertTrue(any("noesis_form" in p for p in self.check([bad], ["T1"])))


class TargetTest(unittest.TestCase):
    def test_unit_and_integration_targets(self):
        unit = {"crate": "aether-noesis", "file": "crates/aether-noesis/src/teach/parity.rs", "name": "x"}
        self.assertEqual(parity.test_target(unit), ("aether-noesis", ["--lib"], "teach::parity"))
        top = {"crate": "aether-noesis", "file": "crates/aether-noesis/src/settings.rs", "name": "x"}
        self.assertEqual(parity.test_target(top)[2], "settings")
        mod = {"crate": "aether-daemon", "file": "crates/aether-daemon/src/noesis/mod.rs", "name": "x"}
        self.assertEqual(parity.test_target(mod)[2], "noesis")
        integ = {"crate": "aether-daemon", "file": "crates/aether-daemon/tests/noesis_guard.rs", "name": "x"}
        self.assertEqual(parity.test_target(integ), ("aether-daemon", ["--test", "noesis_guard"], ""))

    def test_results_match_by_module_path(self):
        out = "test teach::parity::t01 ... ok\ntest settings::tests::bad ... FAILED\ntest other::t01 ... ok\n"
        results = parity.parse_results(out)
        self.assertEqual(parity.outcome(results, "teach::parity", "t01"), "ok")
        self.assertEqual(parity.outcome(results, "settings", "bad"), "FAILED")
        self.assertEqual(parity.outcome(results, "teach::parity", "missing"), "not run")

    def test_run_cases_reports_each_row_with_a_fake_runner(self):
        cases = [
            case("T1", tests=[{"crate": "aether-noesis", "file": "crates/aether-noesis/src/a.rs", "name": "p"}]),
            case("T2", tests=[{"crate": "aether-noesis", "file": "crates/aether-noesis/src/a.rs", "name": "q"}]),
            case("T3", gap="no test"),
        ]
        fake = lambda crate, target, entries: {(c, n): ("ok" if n == "p" else "FAILED") for c, _, n in entries}
        table = parity.run_cases(cases, fake)
        self.assertEqual(table, {"T1": "pass", "T2": "FAIL: q", "T3": "gap"})


class RealManifestTest(unittest.TestCase):
    def test_the_shipped_manifest_passes_the_check(self):
        cases = parity.load_cases()
        self.assertEqual(parity.check_cases(cases), [])

    def test_every_required_id_has_exactly_one_row(self):
        ids = [c["id"] for c in copy.deepcopy(parity.load_cases())]
        self.assertEqual(sorted(ids), sorted(parity.REQUIRED_IDS))


if __name__ == "__main__":
    unittest.main()
