"""The reorder, migrate and stockbook templates: seeds verify, the starter's visible tests pass, a rule broken in the reference fails."""
import random
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from release_bench_families import hard_migrate, hard_reorder, hard_stockbook
from release_bench_generate import verify
from release_bench_tasks import materialize
from test_release_bench_families import grade_with

TEMPLATES = {"reorder": hard_reorder, "migrate": hard_migrate, "stockbook": hard_stockbook}


def make(template, seed):
    return TEMPLATES[template].generate(seed, random.Random(f"{template}:{seed}"))


def broken(spec, suffix, old, new):
    """The reference solution with `old` replaced by `new` in the file ending in `suffix`."""
    path = next(path for path in spec["solution"] if path.endswith(suffix))
    assert old in spec["solution"][path], f"{spec['id']}: {old!r} is not in {path}"
    return {**spec["solution"], path: spec["solution"][path].replace(old, new)}


class HardTemplateTests(unittest.TestCase):
    def test_seeds_verify_and_are_deterministic(self):
        with tempfile.TemporaryDirectory() as scratch:
            for template in TEMPLATES:
                for seed in range(3):
                    spec = make(template, seed)
                    self.assertEqual(spec, make(template, seed))
                    self.assertTrue(6 <= spec["checks"] <= 10, spec["id"])
                    result = verify(spec, scratch=scratch)
                    self.assertTrue(result["ok"], (spec["id"], result["problems"]))

    def test_visible_tests_pass_on_the_starter(self):
        with tempfile.TemporaryDirectory() as root:
            for template in TEMPLATES:
                workspace = materialize(make(template, 1), Path(root, template))
                done = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=workspace,
                                      capture_output=True, text=True, check=False)
                self.assertEqual(done.returncode, 0, f"{template}: {done.stderr}")

    def assert_fails(self, spec, solution):
        with tempfile.TemporaryDirectory() as root:
            result = grade_with(spec, solution, root)
        self.assertLess(result["passed"], result["total"], spec["id"])
        self.assertIsNone(result["error"])

    def test_reorder_that_redelivers_late_events_fails(self):
        for seed in range(3):
            spec = make("reorder", seed)
            self.assert_fails(spec, broken(spec, "buffer.py", hard_reorder.STALE["good"], hard_reorder.STALE["stale"]))

    def test_migration_that_lets_the_first_of_a_tie_win_fails(self):
        for seed in range(3):
            spec = make("migrate", seed)
            self.assert_fails(spec, broken(spec, "migrate.py", "_stamp(new) >= _stamp(old)", "_stamp(new) > _stamp(old)"))

    def test_stock_report_that_ignores_reservations_fails(self):
        for seed in range(3):
            spec = make("stockbook", seed)
            self.assert_fails(spec, broken(spec, "report.py", '"available": item["on_hand"] - item["reserved"]',
                                           '"available": item["on_hand"]'))


if __name__ == "__main__":
    unittest.main()
