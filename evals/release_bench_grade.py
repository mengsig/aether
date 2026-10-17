#!/usr/bin/env python3
"""Hidden-check grading for release-bench tasks.

The hidden unittest module never enters the workspace: it is written beside
it, then run by this file as a subprocess (`--run`), isolated (-I -B), with a
bounded time per check and per grade. The child prints one line,
`RELEASE_BENCH_REPORT:<json>`, naming every check that passed and failed.
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPORT_PREFIX = "RELEASE_BENCH_REPORT:"
GRADE_TIMEOUT_SECONDS = 120
CHECK_TIMEOUT_SECONDS = 10


class CheckTimeout(Exception):
    pass


def grade(workspace, hidden_source, expected_checks, scratch_dir, timeout=GRADE_TIMEOUT_SECONDS):
    """{"passed", "total", "failed": [names], "error": text or None} for one workspace.

    A crash, hang or unreadable report grades 0 of `expected_checks`, with the reason."""
    assert expected_checks > 0, "a task has at least one hidden check"
    workspace = Path(workspace).resolve()
    assert workspace.is_dir(), f"no workspace to grade: {workspace}"
    Path(scratch_dir).mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="grade-", dir=scratch_dir) as holder:
        hidden_path = Path(holder) / "hidden_checks.py"
        hidden_path.write_text(hidden_source, encoding="utf-8")
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "PYTHONHASHSEED": "0",
               "HOME": holder, "TMPDIR": holder, "LC_ALL": "C.UTF-8", "TZ": "UTC"}
        command = [sys.executable, "-I", "-B", os.path.abspath(__file__), "--run", str(workspace), str(hidden_path)]
        try:
            completed = subprocess.run(command, cwd=workspace, env=env, capture_output=True, text=True,
                                       timeout=timeout, check=False, start_new_session=True)
        except subprocess.TimeoutExpired:
            return _failed(expected_checks, f"grading exceeded {timeout}s")
    report = _parse_report(completed.stdout)
    if report is None:
        tail = (completed.stderr or completed.stdout).strip()[-400:]
        return _failed(expected_checks, f"grader exited {completed.returncode} without a report: {tail}")
    total = len(report["passed"]) + len(report["failed"])
    if total != expected_checks:
        return _failed(expected_checks, f"found {total} hidden checks, the task declares {expected_checks}")
    return {"passed": len(report["passed"]), "total": total, "failed": report["failed"], "error": None}


def _failed(total, error):
    return {"passed": 0, "total": total, "failed": ["*"], "error": error}


def _parse_report(stdout):
    lines = [line for line in stdout.splitlines() if line.startswith(REPORT_PREFIX)]
    if len(lines) != 1:
        return None
    try:
        report = json.loads(lines[0][len(REPORT_PREFIX):])
    except ValueError:
        return None
    if not isinstance(report, dict) or not all(isinstance(report.get(k), list) for k in ("passed", "failed")):
        return None
    return report


# ----- child process -------------------------------------------------------


def _run_one(test):
    import signal
    import unittest

    def on_alarm(_signum, _frame):
        raise CheckTimeout(f"check exceeded {CHECK_TIMEOUT_SECONDS}s")

    result = unittest.TestResult()
    previous = signal.signal(signal.SIGALRM, on_alarm)
    signal.alarm(CHECK_TIMEOUT_SECONDS)
    try:
        test.run(result)
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
    return result.wasSuccessful() and result.testsRun == 1


def _flatten(suite):
    import unittest
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from _flatten(item)
        else:
            yield item


def run_hidden(workspace, hidden_path):
    """Child entry: run each hidden check alone and print the report line."""
    import importlib.util
    import unittest

    sys.path.insert(0, workspace)
    os.chdir(workspace)
    spec = importlib.util.spec_from_file_location("hidden_checks", hidden_path)
    module = importlib.util.module_from_spec(spec)
    passed, failed = [], []
    try:
        spec.loader.exec_module(module)
        tests = list(_flatten(unittest.defaultTestLoader.loadTestsFromModule(module)))
    except Exception as error:  # The candidate's import failed: every check fails, by name where known.
        print(f"hidden checks could not load: {type(error).__name__}: {error}", file=sys.stderr)
        tests = []
        failed = [name for name in _declared_names(hidden_path)]
    for test in tests:
        (passed if _run_one(test) else failed).append(test.id().split(".", 1)[-1])
    sys.stdout.flush()
    print(REPORT_PREFIX + json.dumps({"passed": sorted(passed), "failed": sorted(failed)}), flush=True)


def _declared_names(hidden_path):
    """Test method names read from the source, for a module that cannot be imported."""
    import ast
    tree = ast.parse(Path(hidden_path).read_text(encoding="utf-8"))
    return [f"{node.name}.{item.name}" for node in tree.body if isinstance(node, ast.ClassDef)
            for item in node.body if isinstance(item, ast.FunctionDef) and item.name.startswith("test")]


if __name__ == "__main__":
    if len(sys.argv) != 4 or sys.argv[1] != "--run":
        raise SystemExit("usage: release_bench_grade.py --run WORKSPACE HIDDEN_PATH")
    run_hidden(sys.argv[2], sys.argv[3])
