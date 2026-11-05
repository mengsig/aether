#!/usr/bin/env python3
"""Noesis behavioural parity: check the manifest, or run the tests it names.

--check (default) reads files only. --run shells out to cargo, one invocation
per test target, and honours CARGO_TARGET_DIR from the environment.
"""
import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
MANIFEST = HERE / "parity_cases.json"

REQUIRED_IDS = (
    [f"T{n}" for n in range(1, 13)]
    + [f"S{n}" for n in range(1, 5)]
    + [f"S5-G{n:02d}" for n in range(1, 16)]
    + ["S5-H1", "S5-H2", "S5-SETTINGS"]
    + [f"X{n}" for n in range(1, 11)]
)
CRATES = ("aether-noesis", "aether-daemon")
CASE_KEYS = {"id", "behaviour", "noesis_form", "tests", "gap"}


class ManifestError(Exception):
    pass


def load_cases(path=MANIFEST):
    data = json.loads(Path(path).read_text())
    cases = data.get("cases") if isinstance(data, dict) else None
    if not isinstance(cases, list):
        raise ManifestError(f"{path}: expected an object with a 'cases' list")
    return cases


def _check_shape(case):
    problems = []
    cid = case.get("id", "?")
    for extra in sorted(set(case) - CASE_KEYS):
        problems.append(f"{cid}: unknown key {extra!r}")
    form = case.get("noesis_form", "")
    if form != "same" and not form.startswith("differs: "):
        problems.append(f"{cid}: noesis_form must be 'same' or 'differs: <why>'")
    if not case.get("behaviour"):
        problems.append(f"{cid}: behaviour is empty")
    for test in case.get("tests", []):
        if test.get("crate") not in CRATES:
            problems.append(f"{cid}: unknown crate {test.get('crate')!r}")
    return problems


def _has_fn(source, name):
    return re.search(rf"\bfn {re.escape(name)}\s*\(", source) is not None


def check_cases(cases, repo=REPO, required=REQUIRED_IDS):
    """Return a list of problems; empty means the manifest is sound."""
    assert isinstance(cases, list), "cases must be a list"
    assert repo.is_dir(), f"repo root {repo} is not a directory"
    problems = []
    seen = {}
    for case in cases:
        cid = case.get("id")
        seen[cid] = seen.get(cid, 0) + 1
        problems += _check_shape(case)
        if not case.get("tests") and not case.get("gap"):
            problems.append(f"{cid}: no tests and no gap")
        problems += _check_tests(case, repo)
    for cid in required:
        if seen.get(cid, 0) == 0:
            problems.append(f"{cid}: missing from the manifest")
    for cid, count in seen.items():
        if count > 1:
            problems.append(f"{cid}: duplicate id ({count} entries)")
        if cid not in required:
            problems.append(f"{cid}: not a known parity id")
    return problems


def _check_tests(case, repo):
    problems = []
    for test in case.get("tests", []):
        path = repo / test.get("file", "")
        if not path.is_file():
            problems.append(f"{case['id']}: file {test.get('file')} does not exist")
        elif not _has_fn(path.read_text(), test.get("name", "")):
            problems.append(f"{case['id']}: no fn {test.get('name')} in {test['file']}")
    return problems


def test_target(test):
    """Return (crate, cargo target args, module path) for a manifest test."""
    parts = Path(test["file"]).parts
    assert parts[0] == "crates" and parts[1] == test["crate"], f"{test['file']} is not in {test['crate']}"
    if parts[2] == "tests":
        return test["crate"], ["--test", Path(parts[-1]).stem], ""
    assert parts[2] == "src", f"{test['file']} is neither a unit nor an integration test"
    module = [Path(p).stem for p in parts[3:]]
    if module[-1] in ("mod", "lib"):
        module.pop()
    return test["crate"], ["--lib"], "::".join(module)


def group_runs(cases):
    """Map (crate, target args) to the list of (case id, module, test name)."""
    groups = {}
    for case in cases:
        for test in case.get("tests", []):
            crate, target, module = test_target(test)
            groups.setdefault((crate, tuple(target)), []).append((case["id"], module, test["name"]))
    return groups


def parse_results(output):
    """Return {full test path: 'ok' | 'FAILED' | 'ignored'} from libtest output."""
    found = {}
    for match in re.finditer(r"^test (\S+) \.\.\. (\w+)", output, re.M):
        found[match.group(1)] = match.group(2)
    return found


def outcome(results, module, name):
    """Status of one test, matching its module path exactly (inline `tests` mods allowed)."""
    assert name, "a test needs a name"
    wanted = {name} if not module else {f"{module}::{name}", f"{module}::tests::{name}"}
    for path, status in results.items():
        if path in wanted:
            return status
    return "not run"


def run_group(crate, target, entries):
    command = ["cargo", "test", "-p", crate, *target, "--", *sorted({n for _, _, n in entries})]
    done = subprocess.run(command, cwd=REPO, capture_output=True, text=True)
    results = parse_results(done.stdout)
    if not results and done.returncode != 0:
        sys.stderr.write(done.stderr[-2000:])
    return {(cid, name): outcome(results, module, name) for cid, module, name in entries}


def run_cases(cases, runner=run_group):
    """Run every named test; return {case id: 'pass' | 'FAIL: <tests>' | 'gap'}."""
    per_test = {}
    for (crate, target), entries in group_runs(cases).items():
        per_test.update(runner(crate, list(target), entries))
    table = {}
    for case in cases:
        bad = [n for (cid, n), s in per_test.items() if cid == case["id"] and s != "ok"]
        if not case.get("tests"):
            table[case["id"]] = "gap"
        else:
            table[case["id"]] = "pass" if not bad else "FAIL: " + ", ".join(bad)
    assert set(table) == {c["id"] for c in cases}, "every case must get a verdict"
    return table


def print_table(table):
    for cid, verdict in table.items():
        print(f"{cid:<12} {verdict}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="verify the manifest without cargo (default)")
    mode.add_argument("--run", action="store_true", help="run the named tests with cargo")
    args = parser.parse_args(argv)
    cases = load_cases()
    problems = check_cases(cases)
    for problem in problems:
        print(f"PROBLEM: {problem}", file=sys.stderr)
    if problems:
        return 1
    gaps = [c["id"] for c in cases if c.get("gap")]
    if not args.run:
        print(f"ok: {len(cases)} parity rows, {sum(len(c['tests']) for c in cases)} tests, gaps: {gaps or 'none'}")
        return 0
    table = run_cases(cases)
    print_table(table)
    return 1 if any(v.startswith("FAIL") for v in table.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
