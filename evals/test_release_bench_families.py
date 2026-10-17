"""Release bench templates: one seed of each is deterministic, fair and solvable, and grading is strict."""
import re
import tempfile
import unittest
from pathlib import Path

import release_bench_families as families
from release_bench_families.docspec_versions import PRERELEASE_EXAMPLES
from release_bench_generate import verify
from release_bench_grade import grade
from release_bench_tasks import canonical_json, materialize


class TemplateTests(unittest.TestCase):
    def test_a_seed_always_yields_the_same_task(self):
        for template in sorted(families.TEMPLATES):
            with self.subTest(template=template):
                first, again = families.generate(template, 3), families.generate(template, 3)
                self.assertEqual(canonical_json(first), canonical_json(again))
                self.assertNotEqual(canonical_json(first), canonical_json(families.generate(template, 4)))

    def test_every_template_verifies(self):
        with tempfile.TemporaryDirectory() as scratch:
            for template in sorted(families.TEMPLATES):
                with self.subTest(template=template):
                    result = verify(families.generate(template, 0), scratch=scratch)
                    self.assertTrue(result["ok"], result["problems"])
                    self.assertLess(result["starter"]["passed"], result["starter"]["total"])

    def test_candidates_round_robin_over_templates(self):
        specs = families.candidates(len(families.TEMPLATES) * 2, first_seed=5)
        self.assertEqual({spec["template"] for spec in specs}, set(families.TEMPLATES))
        self.assertEqual(len({spec["id"] for spec in specs}), len(specs))


def grade_with(spec, solution, root):
    """Grade the starter with `solution` files written over it."""
    workspace = materialize(spec, Path(root, "workspace"))
    for path, text in solution.items():
        Path(workspace, path).parent.mkdir(parents=True, exist_ok=True)
        Path(workspace, path).write_text(text)
    return grade(workspace, spec["hidden"], spec["checks"], Path(root, "grade"))


class HarnessFamilyTests(unittest.TestCase):
    """The refactor, delegation and long-context templates grade the behaviour they are for."""

    def test_a_consolidation_that_drops_the_exporters_quirks_fails(self):
        for seed in range(3):
            spec = families.generate("exporters", seed)
            exporters = next(path for path in spec["solution"] if path.endswith("/exporters.py"))
            naive = re.sub(r"(columns=\([^)]*\))[^)]*\)", r"\1)", spec["solution"][exporters])
            self.assertNotEqual(naive, spec["solution"][exporters], "the seed has quirks to drop")
            with tempfile.TemporaryDirectory() as root:
                result = grade_with(spec, {**spec["solution"], exporters: naive}, root)
            self.assertLess(result["passed"], result["total"], seed)

    def test_exporters_that_keep_their_own_formatting_fail(self):
        spec = families.generate("exporters", 1)
        rows = {path: text for path, text in spec["solution"].items() if path.endswith("rows.py")}
        with tempfile.TemporaryDirectory() as root:
            result = grade_with(spec, rows, root)
        self.assertLess(result["passed"], result["total"])
        self.assertGreater(result["passed"], 0, "the untouched exporters still produce the right output")

    def test_workstreams_pass_only_with_every_package_fixed(self):
        spec = families.generate("workstreams", 2)
        self.assertTrue(spec["meta"]["allow_delegation"])
        self.assertEqual(len(spec["solution"]), 4)
        for left_out in spec["solution"]:
            partial = {path: text for path, text in spec["solution"].items() if path != left_out}
            with tempfile.TemporaryDirectory() as root:
                result = grade_with(spec, partial, root)
            self.assertLess(result["passed"], result["total"], left_out)

    def test_the_protocol_doc_is_long_and_every_planted_bug_is_checked(self):
        spec = families.generate("protocol", 3)
        doc = next(text for path, text in spec["files"].items() if path.endswith("protocol.md"))
        self.assertGreater(len(doc), 40_000)
        self.assertEqual(spec["checks"], len(spec["meta"]["bugs"].split(",")) + 1)
        with tempfile.TemporaryDirectory() as scratch:
            result = verify(spec, scratch=scratch)
        self.assertTrue(result["ok"], result["problems"])
        self.assertEqual(result["starter"]["passed"], 0)

    def test_the_versions_doc_examples_hold_under_both_policies(self):
        seen = set()
        for seed in range(8):
            spec = families.generate("versions", seed)
            strict = spec["meta"]["strict"]
            seen.add(strict)
            doc = spec["files"]["docs/versions.md"]
            for _, text, _ in PRERELEASE_EXAMPLES[strict]:
                self.assertIn(f"`{text}`", doc)
        self.assertEqual(seen, {True, False})
        self.assertIn(("2.0.0-beta", ">=1.0.0 <2.0.0", True), PRERELEASE_EXAMPLES[False])


class GradeTests(unittest.TestCase):
    HIDDEN = ("import unittest\n\nfrom pkg.mod import value\n\n\nclass Checks(unittest.TestCase):\n"
              "    def test_one(self):\n        self.assertEqual(value(), 1)\n\n"
              "    def test_two(self):\n        self.assertEqual(value() + 1, 2)\n")

    def _workspace(self, root, body):
        spec = {"id": "tiny-0", "family": "f", "template": "tiny", "seed": 0, "prompt": "p", "history": [],
                "tags": {}, "meta": {}, "hidden": self.HIDDEN, "checks": 2, "solution": {"pkg/mod.py": body},
                "files": {"pkg/__init__.py": "", "pkg/mod.py": body}}
        return materialize(spec, Path(root, "workspace"))

    def test_counts_passing_checks(self):
        with tempfile.TemporaryDirectory() as root:
            workspace = self._workspace(root, "def value():\n    return 1\n")
            self.assertEqual(grade(workspace, self.HIDDEN, 2, Path(root, "grade"))["passed"], 2)

    def test_an_import_failure_or_a_wrong_count_grades_zero(self):
        with tempfile.TemporaryDirectory() as root:
            workspace = self._workspace(root, "raise RuntimeError('broken')\n")
            broken = grade(workspace, self.HIDDEN, 2, Path(root, "grade"))
            self.assertEqual(broken["passed"], 0)
            fixed = self._workspace(Path(root, "other"), "def value():\n    return 1\n")
            miscounted = grade(fixed, self.HIDDEN, 3, Path(root, "grade"))
            self.assertEqual(miscounted["passed"], 0)
            self.assertIn("declares 3", miscounted["error"])

    def test_a_hanging_check_is_cut_off(self):
        with tempfile.TemporaryDirectory() as root:
            workspace = self._workspace(root, "def value():\n    while True:\n        pass\n")
            result = grade(workspace, self.HIDDEN, 2, Path(root, "grade"), timeout=60)
            self.assertEqual(result["passed"], 0)
            self.assertEqual(result["total"], 2)


if __name__ == "__main__":
    unittest.main()
