#!/usr/bin/env python3
"""Noesis behavioural parity: check the manifest, or run the tests it names.

Each row quotes its NOESIS.md §6 (or ARCHITECTURE.md §g) behaviour and splits
it into clauses; every clause names the tests that prove it, records a gap,
or both. --check (default) reads files only and lists every gap. --run shells
out to cargo, one invocation per test target, and honours CARGO_TARGET_DIR.
"""
import argparse
import json
import re
import shlex
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
    + ["B01", "B02"]
)
CRATES = ("aether-noesis", "aether-daemon", "aether-protocol")
CASE_KEYS = {"id", "behaviour", "noesis_form", "clauses"}
CLAUSE_KEYS = {"clause", "tests", "gap", "noesis"}
TEST_KEYS = {"crate", "file", "name"}
OUTPUT_TAIL_CHARS = 4000


class ManifestError(Exception):
    pass


def load_cases(path=MANIFEST):
    data = json.loads(Path(path).read_text())
    cases = data.get("cases") if isinstance(data, dict) else None
    if not isinstance(cases, list):
        raise ManifestError(f"{path}: expected an object with a 'cases' list")
    return cases


def clauses_of(case):
    clauses = case.get("clauses")
    return [c for c in clauses if isinstance(c, dict)] if isinstance(clauses, list) else []


def case_tests(case):
    """Every test entry of a row, across its clauses, in manifest order."""
    tests = []
    for clause in clauses_of(case):
        listed = clause.get("tests", [])
        tests += [t for t in listed if isinstance(t, dict)] if isinstance(listed, list) else []
    return tests


def _words(text):
    return set(re.findall(r"\w+", text.lower()))


def _check_shape(case):
    cid = case.get("id", "?")
    problems = [f"{cid}: unknown key {extra!r}" for extra in sorted(set(case) - CASE_KEYS)]
    behaviour, form = case.get("behaviour"), case.get("noesis_form")
    for key, value in (("behaviour", behaviour), ("noesis_form", form)):
        if not isinstance(value, str) or not value.strip():
            problems.append(f"{cid}: {key} is empty")
    clauses = case.get("clauses")
    if not isinstance(clauses, list) or not clauses:
        problems.append(f"{cid}: needs a non-empty 'clauses' list")
    if problems:
        return problems
    for clause in clauses:
        problems += _check_clause(cid, clause, behaviour, form)
    return problems + _check_coverage(cid, clauses, behaviour, form)


def _check_clause(cid, clause, behaviour, form):
    if not isinstance(clause, dict) or not isinstance(clause.get("clause"), str) or not clause["clause"].strip():
        return [f"{cid}: a clause needs a non-empty 'clause' text"]
    text = clause["clause"]
    problems = [f"{cid}: clause {text!r} has unknown key {k!r}" for k in sorted(set(clause) - CLAUSE_KEYS)]
    if text not in behaviour and text not in form:
        problems.append(f"{cid}: clause {text!r} is not quoted from the behaviour or the Noesis form")
    for key in ("gap", "noesis"):
        if key in clause and (not isinstance(clause[key], str) or not clause[key].strip()):
            problems.append(f"{cid}: clause {text!r} has an empty {key!r}")
    tests = clause.get("tests", [])
    if not isinstance(tests, list):
        return problems + [f"{cid}: clause {text!r} 'tests' is not a list"]
    if not tests and not clause.get("gap"):
        problems.append(f"{cid}: clause {text!r} has no tests and no gap")
    for test in tests:
        if not isinstance(test, dict) or set(test) != TEST_KEYS:
            problems.append(f"{cid}: clause {text!r} has a test entry without exactly {sorted(TEST_KEYS)}")
        elif test["crate"] not in CRATES:
            problems.append(f"{cid}: unknown crate {test['crate']!r}")
    return problems


def _check_coverage(cid, clauses, behaviour, form):
    """Every word of the behaviour is in a clause, and a non-'same' form is accounted for."""
    covered = set().union(*(_words(c["clause"]) for c in clauses if isinstance(c, dict) and "clause" in c))
    problems = []
    missing = sorted(_words(behaviour) - covered)
    if missing:
        problems.append(f"{cid}: behaviour words in no clause: {', '.join(missing)}")
    from_form = any(isinstance(c, dict) and (c.get("noesis") or c.get("clause", "") in form) for c in clauses)
    if form != "same" and not from_form:
        problems.append(f"{cid}: the Noesis form {form!r} is in no clause and no 'noesis' note")
    return problems


def _has_fn(source, name):
    return re.search(rf"\bfn {re.escape(name)}\s*\(", source) is not None


def _declaring_files(repo, file):
    """Return (module name, files one of which must declare it), or None for a crate root or `tests/` file."""
    parts = Path(file).parts
    if len(parts) < 4 or parts[2] != "src":
        return None
    rel = Path(*parts[3:])
    if rel.stem == "mod":
        rel = rel.parent
    if rel.parent == Path(".") and rel.stem in ("lib", "main"):
        return None
    src = repo / parts[0] / parts[1] / "src"
    if rel.parent == Path("."):
        return rel.stem, [src / "lib.rs", src / "main.rs"]
    return rel.stem, [src / rel.parent / "mod.rs", src / rel.parent.with_suffix(".rs")]


def _module_problem(repo, cid, file):
    """The derived module path is only right if the parent declares `mod <stem>` (no `#[path]`)."""
    declared = _declaring_files(repo, file)
    if declared is None:
        return None
    stem, candidates = declared
    pattern = re.compile(rf"^\s*(pub(\([^)]*\))?\s+)?mod\s+{re.escape(stem)}\s*;", re.M)
    if any(path.is_file() and pattern.search(path.read_text()) for path in candidates):
        return None
    return f"{cid}: {file} is not declared as `mod {stem};` by its parent, so its test path is unknown"


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
    cid = case.get("id", "?")
    for test in case_tests(case):
        file, name = test.get("file", ""), test.get("name", "")
        path = repo / file
        if not path.is_file():
            problems.append(f"{cid}: file {file} does not exist")
        elif not _has_fn(path.read_text(), name):
            problems.append(f"{cid}: no fn {name} in {file}")
        else:
            problems += [p for p in [_module_problem(repo, cid, file)] if p]
    return problems


def gap_report(cases):
    """Return (row id, clause, gap) for every clause that records a gap."""
    gaps = [(case["id"], c["clause"], c["gap"]) for case in cases for c in clauses_of(case) if c.get("gap")]
    assert all(cid and clause and gap for cid, clause, gap in gaps), "a gap names its row and clause"
    return gaps


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


def candidate_paths(module, name):
    """The libtest paths a test may have: in its module, or in that module's inline `tests`."""
    assert name and "::" not in name, f"a test name is a bare fn name, got {name!r}"
    if not module:
        return [name]
    return [f"{module}::{name}", f"{module}::tests::{name}"]


def group_runs(cases):
    """Map (crate, target args) to the list of (case id, module, test name)."""
    groups = {}
    for case in cases:
        for test in case_tests(case):
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
    """Status of one test, matched on its full path; two matching paths are ambiguous."""
    found = [results[path] for path in candidate_paths(module, name) if path in results]
    if len(found) > 1:
        return "ambiguous"
    return found[0] if found else "not run"


def cargo_command(crate, target, entries):
    """`cargo test` for exactly the listed tests: `--exact` full paths, so nothing else runs."""
    filters = sorted({path for _, module, name in entries for path in candidate_paths(module, name)})
    assert filters, "a cargo run names at least one test"
    return ["cargo", "test", "-p", crate, *target, "--", "--exact", *filters]


def output_tail(done):
    return ((done.stdout or "") + (done.stderr or ""))[-OUTPUT_TAIL_CHARS:]


def run_group(crate, target, entries, run=subprocess.run):
    """Run one target's tests; any nonzero cargo exit fails every test of the group."""
    command = cargo_command(crate, target, entries)
    done = run(command, cwd=REPO, capture_output=True, text=True)
    assert isinstance(done.returncode, int), "the runner returns an exit status"
    results = parse_results(done.stdout or "")
    statuses = {(cid, name): outcome(results, module, name) for cid, module, name in entries}
    if done.returncode == 0:
        return statuses
    sys.stderr.write(f"{shlex.join(command)} exited {done.returncode}; output tail:\n{output_tail(done)}\n")
    failed = f"exit {done.returncode}"
    return {key: (failed if status == "ok" else status) for key, status in statuses.items()}


def verdict(case, per_test):
    """'pass', 'pass, N gap(s)', 'gap' (no test at all) or 'FAIL: <test (status)>, …'."""
    names = sorted({test["name"] for test in case_tests(case)})
    gaps = sum(1 for clause in clauses_of(case) if clause.get("gap"))
    if not names:
        assert gaps, f"{case['id']}: a row without tests is all gaps"
        return "gap"
    statuses = {name: per_test.get((case["id"], name), "not run") for name in names}
    bad = [f"{name} ({status})" for name, status in statuses.items() if status != "ok"]
    if bad:
        return "FAIL: " + ", ".join(bad)
    return f"pass, {gaps} gap(s)" if gaps else "pass"


def run_cases(cases, runner=run_group):
    """Run every named test; return {case id: verdict}."""
    per_test = {}
    for (crate, target), entries in group_runs(cases).items():
        per_test.update(runner(crate, list(target), entries))
    table = {case["id"]: verdict(case, per_test) for case in cases}
    assert set(table) == {c["id"] for c in cases}, "every case must get a verdict"
    return table


def print_table(table):
    for cid, result in table.items():
        print(f"{cid:<12} {result}")


def print_summary(cases):
    gaps = gap_report(cases)
    clauses = sum(len(clauses_of(case)) for case in cases)
    tests = sum(len(case_tests(case)) for case in cases)
    rows = len({cid for cid, _, _ in gaps})
    gap_text = f"{len(gaps)} clause(s) in {rows} row(s)" if gaps else "none"
    print(f"ok: {len(cases)} parity rows, {clauses} clauses, {tests} test references; gaps: {gap_text}")
    for cid, clause, gap in gaps:
        print(f"GAP {cid}: {clause} -- {gap}")


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
    if not args.run:
        print_summary(cases)
        return 0
    table = run_cases(cases)
    print_table(table)
    return 1 if any(v.startswith("FAIL") for v in table.values()) else 0


if __name__ == "__main__":
    sys.exit(main())
