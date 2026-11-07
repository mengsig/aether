"""Tests of the parity manifest checker and runner. No cargo, no model: the runner gets a fake subprocess."""
import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import parity

SRC = "crates/aether-noesis/src/a.rs"


def entry(name, file=SRC):
    return {"crate": "aether-noesis", "file": file, "name": name}


def case(cid, clauses, behaviour="b", form="same"):
    return {"id": cid, "behaviour": behaviour, "noesis_form": form, "clauses": clauses}


def clause(text="b", tests=(), **extra):
    return dict({"clause": text, "tests": list(tests)}, **extra)


def completed(code, stdout="", stderr=""):
    return subprocess.CompletedProcess(["cargo"], code, stdout=stdout, stderr=stderr)


class CheckerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        src = self.repo / "crates" / "aether-noesis" / "src"
        (src / "teach").mkdir(parents=True)
        (src / "lib.rs").write_text("pub mod a;\npub mod teach;\n")
        (src / "a.rs").write_text("#[test]\nfn present() {}\nasync fn also_present() {}\n")
        (src / "teach" / "mod.rs").write_text('#[cfg(test)]\n#[path = "race_tests.rs"]\nmod race;\n')
        (src / "teach" / "race_tests.rs").write_text("#[test]\nfn raced() {}\n")

    def check(self, cases, required=("T1",)):
        return parity.check_cases(cases, self.repo, list(required))

    def test_a_sound_manifest_passes(self):
        cases = [case("T1", [clause(tests=[entry("present"), entry("also_present")])])]
        self.assertEqual(self.check(cases), [])

    def test_a_missing_test_name_or_file_is_reported(self):
        problems = self.check([case("T1", [clause(tests=[entry("gone")])])])
        self.assertEqual(len(problems), 1)
        self.assertIn("no fn gone", problems[0])
        missing = self.check([case("T1", [clause(tests=[entry("present", "crates/aether-noesis/src/none.rs")])])])
        self.assertIn("does not exist", missing[0])

    def test_a_duplicate_a_missing_and_an_unknown_id_are_reported(self):
        gap = [clause(gap="why")]
        self.assertTrue(any("duplicate id" in p for p in self.check([case("T1", gap), case("T1", gap)])))
        problems = self.check([case("Z9", gap)])
        self.assertTrue(any("T1: missing" in p for p in problems))
        self.assertTrue(any("Z9: not a known" in p for p in problems))

    def test_a_clause_needs_tests_or_a_gap(self):
        self.assertTrue(any("no tests and no gap" in p for p in self.check([case("T1", [clause()])])))
        self.assertEqual(self.check([case("T1", [clause(gap="no test covers it")])]), [])

    def test_a_clause_must_be_quoted_from_the_spec_text(self):
        paraphrase = case("T1", [clause("keeps what it spent", gap="g")], behaviour="keeps its partial output")
        problems = self.check([paraphrase])
        self.assertTrue(any("is not quoted" in p for p in problems), problems)

    def test_every_behaviour_word_must_be_in_a_clause(self):
        dropped = case("T1", [clause("whole file", gap="g")], behaviour="whole file, 3 in flight, 25 MiB limit")
        problems = self.check([dropped])
        self.assertTrue(any("behaviour words in no clause: 25, 3, flight, in, limit, mib" in p for p in problems), problems)

    def test_a_differing_form_needs_a_clause_or_a_note(self):
        silent = case("T1", [clause(gap="g")], form="JSON DAG instead of Mermaid")
        self.assertTrue(any("Noesis form" in p for p in self.check([silent])))
        noted = case("T1", [clause(gap="g", noesis="differs: a JSON DAG")], form="JSON DAG instead of Mermaid")
        self.assertEqual(self.check([noted]), [])
        quoted = case("T1", [clause(gap="g"), clause("JSON DAG", gap="g")], form="JSON DAG instead of Mermaid")
        self.assertEqual(self.check([quoted]), [])

    def test_a_test_file_mounted_with_a_path_attribute_is_refused(self):
        raced = entry("raced", "crates/aether-noesis/src/teach/race_tests.rs")
        problems = self.check([case("T1", [clause(tests=[raced])])])
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("not declared as `mod race_tests;`", problems[0])

    def test_unknown_keys_and_a_bad_crate_are_reported(self):
        bad = dict(entry("present"), crate="aether-core")
        problems = self.check([case("T1", [clause(tests=[bad], extra="x")])])
        self.assertTrue(any("unknown key 'extra'" in p for p in problems), problems)
        self.assertTrue(any("unknown crate" in p for p in problems), problems)

    def test_the_gap_report_names_each_gapped_clause(self):
        cases = [case("T1", [clause("a", tests=[entry("present")]), clause("b", gap="why")], behaviour="a b")]
        self.assertEqual(parity.gap_report(cases), [("T1", "b", "why")])


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

    def test_results_match_by_full_path_only(self):
        out = ("test teach::parity::t01 ... ok\ntest settings::tests::bad ... FAILED\n"
               "test other::t01 ... ok\ntest teach::parity::t01_longer ... ok\n")
        results = parity.parse_results(out)
        self.assertEqual(parity.outcome(results, "teach::parity", "t01"), "ok")
        self.assertEqual(parity.outcome(results, "settings", "bad"), "FAILED")
        self.assertEqual(parity.outcome(results, "teach::parity", "t01_long"), "not run")
        both = parity.parse_results("test m::x ... ok\ntest m::tests::x ... ok\n")
        self.assertEqual(parity.outcome(both, "m", "x"), "ambiguous")


class RunnerTest(unittest.TestCase):
    ENTRIES = [("T1", "teach::parity", "t01"), ("T2", "teach::parity", "t02")]

    def run_group(self, done):
        calls = []

        def fake(command, **kwargs):
            calls.append((command, kwargs))
            return done

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            statuses = parity.run_group("aether-noesis", ["--lib"], self.ENTRIES, run=fake)
        self.assertEqual(len(calls), 1, "one cargo run per target")
        return statuses, calls[0][0], stderr.getvalue()

    def test_filters_are_exact_full_paths_so_no_other_test_runs(self):
        ok = "test teach::parity::t01 ... ok\ntest teach::parity::t02 ... ok\n"
        statuses, command, _ = self.run_group(completed(0, ok))
        self.assertEqual(command[:5], ["cargo", "test", "-p", "aether-noesis", "--lib"])
        filters = command[command.index("--") + 1:]
        self.assertEqual(filters[0], "--exact")
        self.assertEqual(filters[1:], ["teach::parity::t01", "teach::parity::t02",
                                       "teach::parity::tests::t01", "teach::parity::tests::t02"])
        self.assertEqual(statuses, {("T1", "t01"): "ok", ("T2", "t02"): "ok"})

    def test_a_nonzero_exit_fails_tests_whose_ok_lines_were_printed(self):
        out = "test teach::parity::t01 ... ok\ntest teach::parity::t02 ... FAILED\n"
        statuses, _, stderr = self.run_group(completed(101, out, "error: test failed, to rerun pass `--lib`"))
        self.assertEqual(statuses, {("T1", "t01"): "exit 101", ("T2", "t02"): "FAILED"})
        self.assertIn("exited 101", stderr)
        self.assertIn("error: test failed", stderr, "the output tail is kept")

    def test_a_nonzero_exit_with_only_ok_lines_is_still_a_failure(self):
        out = "test teach::parity::t01 ... ok\ntest teach::parity::t02 ... ok\n"
        statuses, _, stderr = self.run_group(completed(101, out, "thread panicked at 'boom'"))
        self.assertTrue(all(s == "exit 101" for s in statuses.values()), statuses)
        self.assertIn("boom", stderr)
        table = parity.run_cases(
            [case("T1", [clause(tests=[entry("t01", "crates/aether-noesis/src/teach/parity.rs")])])],
            lambda crate, target, entries: statuses,
        )
        self.assertEqual(table, {"T1": "FAIL: t01 (exit 101)"})

    def test_run_cases_reports_pass_gaps_and_failures(self):
        cases = [
            case("T1", [clause(tests=[entry("p")])]),
            case("T2", [clause(tests=[entry("q")])]),
            case("T3", [clause(gap="no test")]),
            case("T4", [clause("a", tests=[entry("p")]), clause("b", gap="why")], behaviour="a b"),
        ]
        fake = lambda crate, target, entries: {(c, n): ("ok" if n == "p" else "FAILED") for c, _, n in entries}
        table = parity.run_cases(cases, fake)
        self.assertEqual(table, {"T1": "pass", "T2": "FAIL: q (FAILED)", "T3": "gap", "T4": "pass, 1 gap(s)"})


class RealManifestTest(unittest.TestCase):
    def setUp(self):
        self.cases = parity.load_cases()
        self.by_id = {c["id"]: c for c in self.cases}

    def test_the_shipped_manifest_passes_the_check(self):
        self.assertEqual(parity.check_cases(self.cases), [])

    def test_every_required_id_has_exactly_one_row(self):
        self.assertEqual(sorted(self.by_id), sorted(parity.REQUIRED_IDS))
        self.assertEqual(len(self.cases), len(self.by_id))

    def test_known_unmapped_clauses_are_reported_as_gaps(self):
        gapped = {(cid, text) for cid, text, _ in parity.gap_report(self.cases)}
        for expected in [
            ("S1", "One turn per session: a concurrent turn is refused"),
            ("S1", "input is persisted before inference"),
            ("S1", "and an aborted turn keeps its partial output"),
            ("X8", "3 in flight"),
            ("X8", "25 MiB limit"),
        ]:
            self.assertIn(expected, gapped)

    def test_the_t8_difference_is_recorded(self):
        t8 = self.by_id["T8"]
        self.assertEqual(t8["noesis_form"], "JSON DAG instead of Mermaid")
        notes = [c.get("noesis", "") for c in t8["clauses"]]
        self.assertTrue(any(n.startswith("differs:") and "Mermaid" in n for n in notes), notes)

    def test_the_check_prints_the_gap_count(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(parity.main(["--check"]), 0)
        gaps = len(parity.gap_report(self.cases))
        self.assertGreater(gaps, 0)
        self.assertIn(f"gaps: {gaps} clause(s)", out.getvalue())
        self.assertEqual(out.getvalue().count("\nGAP "), gaps)


if __name__ == "__main__":
    unittest.main()
