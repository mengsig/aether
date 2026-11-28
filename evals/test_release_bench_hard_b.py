"""Hard release-bench templates (pagination, prorate, jobgraph): fair, deterministic, strict."""
import random
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import release_bench_families.hard_jobgraph as jobgraph
import release_bench_families.hard_pagination as pagination
import release_bench_families.hard_prorate as prorate
from release_bench_generate import verify
from release_bench_grade import grade
from release_bench_tasks import canonical_json, materialize

MODULES = {"pagination": pagination, "prorate": prorate, "jobgraph": jobgraph}


def make(template, seed):
    return MODULES[template].generate(seed, random.Random(f"{template}:{seed}"))


def grade_with(spec, solution, root):
    """Grade the starter with `solution` files written over it."""
    workspace = materialize(spec, Path(root, "workspace"))
    for path, text in solution.items():
        Path(workspace, path).write_text(text)
    return grade(workspace, spec["hidden"], spec["checks"], Path(root, "grade"))


def break_rule(spec, suffix, old, new):
    """The reference solution with one rule broken in the file ending in `suffix`."""
    path = next(path for path in spec["solution"] if path.endswith(suffix))
    text = spec["solution"][path]
    assert old in text, f"{old!r} not found in {path}"
    return {**spec["solution"], path: text.replace(old, new)}


class HardTemplateTests(unittest.TestCase):
    def test_seeds_verify_and_repeat_exactly(self):
        with tempfile.TemporaryDirectory() as scratch:
            for template in MODULES:
                for seed in range(3):
                    spec = make(template, seed)
                    self.assertEqual(canonical_json(spec), canonical_json(make(template, seed)))
                    self.assertTrue(6 <= spec["checks"] <= 10, (template, spec["checks"]))
                    result = verify(spec, scratch=scratch)
                    self.assertTrue(result["ok"], (template, seed, result["problems"]))

    def test_visible_tests_pass_on_the_starter(self):
        for template in MODULES:
            with tempfile.TemporaryDirectory() as root:
                workspace = materialize(make(template, 1), Path(root, "workspace"))
                done = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=workspace,
                                      capture_output=True, text=True)
                self.assertEqual(done.returncode, 0, f"{template}: {done.stderr[-500:]}")

    def test_pagination_with_a_wrong_has_more_fails(self):
        spec = make("pagination", 0)
        broken = break_rule(spec, "api.py", "more = start + limit < len(rows)", "more = len(page) == limit")
        with tempfile.TemporaryDirectory() as root:
            result = grade_with(spec, broken, root)
        self.assertLess(result["passed"], result["total"])

    def test_prorate_that_counts_days_outside_the_period_fails(self):
        spec = make("prorate", 0)
        broken = break_rule(spec, "proration.py", "max(start, period_start), min(end, period_end)", "start, end")
        with tempfile.TemporaryDirectory() as root:
            result = grade_with(spec, broken, root)
        self.assertLess(result["passed"], result["total"])

    def test_jobgraph_that_skips_only_direct_dependents_fails(self):
        spec = make("jobgraph", 0)
        broken = break_rule(spec, "runner.py", "skipped.get(dep, [])", "[]")
        with tempfile.TemporaryDirectory() as root:
            result = grade_with(spec, broken, root)
        self.assertLess(result["passed"], result["total"])


if __name__ == "__main__":
    unittest.main()
