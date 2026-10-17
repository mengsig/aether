#!/usr/bin/env python3
"""Generate release-bench candidates and prove each one is a fair task.

A candidate is kept only when its starter fails at least one hidden check,
its reference solution passes every one, and grading each twice gives the
same verdict check by check (the checks are deterministic).

Usage:
  python3 evals/release_bench_generate.py --count 200 [--first-seed 0] [--output FILE]
"""
import argparse
import os
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import release_bench_families as families
from release_bench_grade import grade
from release_bench_tasks import (
    apply_solution,
    canonical_json,
    materialize,
    validate_spec,
)

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRATCH = REPO_ROOT / "target" / "release-bench" / "verify"


def _grade_twice(spec, workspace, scratch):
    first = grade(workspace, spec["hidden"], spec["checks"], scratch)
    second = grade(workspace, spec["hidden"], spec["checks"], scratch)
    return first, first == second


def verify(spec, scratch=SCRATCH):
    """{"ok", "problems", "starter", "solution"}: whether `spec` is fair, deterministic and solvable."""
    validate_spec(spec)
    Path(scratch).mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix=f"{spec['id']}-", dir=scratch))
    problems = []
    try:
        workspace = materialize(spec, root / "workspace")
        starter, starter_stable = _grade_twice(spec, workspace, root)
        apply_solution(spec, workspace)
        solution, solution_stable = _grade_twice(spec, workspace, root)
    finally:
        shutil.rmtree(root, ignore_errors=False)
    if starter["passed"] >= starter["total"]:
        problems.append("the starter already passes every hidden check")
    if solution["passed"] != solution["total"] or solution["error"]:
        problems.append(f"the reference solution fails {solution['failed']}: {solution['error']}")
    if not (starter_stable and solution_stable):
        problems.append("grading the same files twice gave different results")
    return {"ok": not problems, "problems": problems, "starter": starter, "solution": solution}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--count", type=int, default=200)
    parser.add_argument("--first-seed", type=int, default=0)
    parser.add_argument("--templates", default=None, help="comma-separated template names (default: all)")
    parser.add_argument("--output", type=Path, default=None, help="write verified specs here as JSON lines")
    args = parser.parse_args(argv)
    if args.count <= 0 or args.first_seed < 0:
        parser.error("--count must be positive and --first-seed non-negative")
    templates = args.templates.split(",") if args.templates else None
    specs, failures = [], 0
    for spec in families.candidates(args.count, args.first_seed, templates):
        result = verify(spec)
        status = "ok" if result["ok"] else "REJECTED " + "; ".join(result["problems"])
        print(f"{spec['id']:<24} starter {result['starter']['passed']}/{result['starter']['total']}  {status}")
        failures += not result["ok"]
        if result["ok"]:
            specs.append(spec)
    if args.output:
        args.output.write_text("".join(canonical_json(spec) + "\n" for spec in specs), encoding="utf-8")
    print(f"{len(specs)} verified, {failures} rejected")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
