"""Release bench templates: one seed of each is deterministic, fair and solvable, and grading is strict."""
import tempfile
import unittest
from pathlib import Path

import release_bench_families as families
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
