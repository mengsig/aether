"""The bundle template of set v3: several hard tasks in one repository, graded part by part."""
import tempfile
import unittest
from pathlib import Path

import release_bench_families as families
from release_bench_families.hard_bundle import PARTS
from release_bench_generate import verify
from release_bench_grade import grade
from release_bench_tasks import apply_solution, canonical_json, materialize


def parts_of(spec):
    """{part id: its own spec} of a bundle."""
    parts = {}
    for part in spec["meta"]["parts"].split(","):
        template, seed = part.rsplit("-", 1)
        parts[part] = families.generate(template, int(seed))
    return parts


class BundleTests(unittest.TestCase):
    def test_a_bundle_holds_distinct_hard_parts_and_all_their_checks(self):
        for seed in range(6):
            spec = families.generate("bundle", seed)
            parts = parts_of(spec)
            self.assertEqual(len(parts), PARTS)
            self.assertEqual(len({part.split("-")[0] for part in parts}), PARTS, "no template twice")
            self.assertTrue(set(part.split("-")[0] for part in parts) <= set(families.HARD_TEMPLATES))
            self.assertEqual(spec["checks"], sum(part["checks"] for part in parts.values()))
            for index, part in enumerate(parts.values(), 1):
                self.assertIn(f"{index}. {part['prompt'].strip()}", spec["prompt"])
            self.assertEqual(canonical_json(spec), canonical_json(families.generate("bundle", seed)))

    def test_bundles_are_fair(self):
        for seed in (0, 1):
            with self.subTest(seed=seed):
                result = verify(families.generate("bundle", seed))
                self.assertTrue(result["ok"], result["problems"])

    def test_a_part_that_no_longer_imports_fails_only_its_own_checks(self):
        spec = families.generate("bundle", 1)
        parts = parts_of(spec)
        broken_id, broken = next(item for item in parts.items() if item[0].startswith("prorate"))
        package = next(path for path in broken["solution"] if path.endswith(".py")).split("/")[0]
        with tempfile.TemporaryDirectory() as root:
            workspace = materialize(spec, Path(root, "workspace"))
            apply_solution(spec, workspace)
            Path(workspace, package, "__init__.py").write_text("raise ImportError('broken on purpose')\n")
            result = grade(workspace, spec["hidden"], spec["checks"], Path(root, "grade"))
        self.assertIsNone(result["error"])
        self.assertEqual(result["total"], spec["checks"])
        self.assertEqual(len(result["failed"]), broken["checks"], f"only {broken_id} fails")
        label = broken_id.replace("-", "_")
        self.assertTrue(all(name.startswith(label) for name in result["failed"]), result["failed"])

    def test_a_part_s_history_and_tags_survive_the_merge(self):
        spec = next(spec for spec in (families.generate("bundle", seed) for seed in range(20))
                    if "pagination" in spec["meta"]["parts"])
        pagination = next(part for name, part in parts_of(spec).items() if name.startswith("pagination"))
        self.assertEqual(spec["history"], pagination["history"])
        self.assertEqual(spec["tags"], pagination["tags"])
        self.assertTrue(all(part["files"].keys() - {"README.md"} <= spec["files"].keys()
                            for part in parts_of(spec).values()))


if __name__ == "__main__":
    unittest.main()
