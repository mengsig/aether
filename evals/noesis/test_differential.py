"""Tests of the differential harness that need no checkout: skipping, comparator, faux model."""
import copy
import json
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent / "differential"
sys.path.insert(0, str(HERE))
import adapter
import compare
import run
from faux_model import FauxModel, sse_chunks

RUN = HERE / "run.py"

# A launcher that forks a sleeping child, waits until the child wrote its pid, then exits.
LAUNCHER = """
import os, signal, sys, time
pid_file, mode = sys.argv[1], sys.argv[2]
if os.fork() == 0:
    if mode == "stubborn":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    with open(pid_file + ".tmp", "w") as out:
        out.write(str(os.getpid()))
    os.rename(pid_file + ".tmp", pid_file)
    time.sleep(300)
    os._exit(0)
deadline = time.monotonic() + 10
while not os.path.exists(pid_file) and time.monotonic() < deadline:
    time.sleep(0.01)
"""


def process_alive(pid):
    """True while `pid` runs; a zombie counts as gone where /proc can tell."""
    stat = Path(f"/proc/{pid}/stat")
    if Path("/proc/self/stat").is_file():
        try:
            return stat.read_text().rsplit(")", 1)[1].split()[0] != "Z"
        except FileNotFoundError:
            return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def wait_gone(pid, seconds=5):
    deadline = time.monotonic() + seconds
    while process_alive(pid) and time.monotonic() < deadline:
        time.sleep(0.02)
    return not process_alive(pid)


def kill_recorded(pid):
    """Test safety net: kill only the pid the launcher recorded, if it is still there."""
    if process_alive(pid):
        os.kill(pid, signal.SIGKILL)


def run_harness(*args, env_extra=None):
    env = {k: v for k, v in os.environ.items() if k != "LEARNHARNESS_DIR"}
    env.update(env_extra or {})
    return subprocess.run([sys.executable, str(RUN), *args], capture_output=True, text=True, env=env, timeout=30)


class SkipTest(unittest.TestCase):
    def test_no_checkout_skips_with_exit_zero(self):
        done = run_harness()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertTrue(done.stdout.startswith("SKIP"), done.stdout)

    def test_a_directory_that_is_not_a_checkout_skips(self):
        with tempfile.TemporaryDirectory() as empty:
            done = run_harness("--learnharness", empty)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("SKIP", done.stdout)

    def test_a_missing_path_from_the_environment_skips(self):
        done = run_harness(env_extra={"LEARNHARNESS_DIR": "/nonexistent/learnharness"})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("SKIP", done.stdout)


class ComparatorTest(unittest.TestCase):
    def setUp(self):
        self.expected = json.loads((HERE / "expected" / "structure.json").read_text())

    def test_identical_structure_matches_despite_case_order_and_spacing(self):
        actual = copy.deepcopy(self.expected)
        actual["graph"]["statements"] = ["The  slope of x^2 at 1 is 2", "A derivative is a limit of slopes",
                                         "The derivative of x^2 is 2X"]
        actual["graph"]["premise_sets"][0]["premises"].reverse()
        actual["graph"]["premise_sets"][0]["conclusion"] = " The slope of x^2 at 1 is 2 "
        self.assertEqual(compare.differences(self.expected, actual), [])

    def test_a_phase_question_plan_or_graph_difference_is_named(self):
        actual = copy.deepcopy(self.expected)
        actual["phases"] = ["probe", "teach"]
        actual["questions"][0]["valid"] = True
        actual["plan_validations"][1]["valid"] = False
        actual["graph"]["premise_sets"] = []
        keys = [line.split(":")[0] for line in compare.differences(self.expected, actual)]
        self.assertEqual(keys, ["phases", "questions", "plan_validations", "graph"])

    def test_a_trace_missing_a_section_is_an_error(self):
        broken = {k: v for k, v in self.expected.items() if k != "graph"}
        with self.assertRaises(compare.TraceError):
            compare.differences(self.expected, broken)


class AdapterAndModelTest(unittest.TestCase):
    def test_state_reduces_to_a_trace_and_collapses_repeated_phases(self):
        state = {"phase_history": ["probe", "probe", "plan"], "questions": [{"valid": True}],
                 "plan_attempts": [{"valid": False, "problems": ["cycle"]}],
                 "graph": {"statements": ["s"], "premise_sets": []}}
        trace = adapter.trace_from_state(state)
        self.assertEqual(trace["phases"], ["probe", "plan"])
        self.assertEqual(trace["plan_validations"], [{"valid": False, "problems": ["cycle"]}])

    def test_a_state_that_breaks_the_assumptions_fails_loudly(self):
        with self.assertRaises(adapter.AdapterError):
            adapter.trace_from_state({"phase_history": []})

    def test_checkout_detection(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertFalse(adapter.looks_like_checkout(tmp))
            Path(tmp, "package.json").write_text("{}")
            self.assertTrue(adapter.looks_like_checkout(tmp))
        self.assertFalse(adapter.looks_like_checkout(""))

    def test_a_tool_turn_streams_a_call_then_a_finish_reason(self):
        chunks = sse_chunks({"tool_calls": [{"name": "noesis.plan", "arguments": {"nodes": []}}]})
        call = chunks[0]["choices"][0]["delta"]["tool_calls"][0]["function"]
        self.assertEqual(call["name"], "noesis.plan")
        self.assertEqual(json.loads(call["arguments"]), {"nodes": []})
        self.assertEqual(chunks[-1]["choices"][0]["finish_reason"], "tool_calls")

    def test_the_faux_server_streams_scripted_turns_then_counts_overruns(self):
        model = FauxModel([{"content": "hello"}])
        url = model.start()
        self.addCleanup(model.stop)
        self.assertTrue(url.startswith("http://127.0.0.1:"))
        bodies = []
        for _ in range(2):
            request = urllib.request.Request(url + "/chat/completions", data=b"{}", method="POST")
            with urllib.request.urlopen(request, timeout=10) as reply:
                bodies.append(reply.read().decode())
        self.assertIn("hello", bodies[0])
        self.assertTrue(bodies[0].rstrip().endswith("data: [DONE]"))
        self.assertEqual((model.requests, model.overruns), (2, 1))

    def test_the_script_and_expectation_agree_on_shape(self):
        script = json.loads((HERE / "script.json").read_text())
        self.assertGreaterEqual(len(script["model_turns"]), 5)
        compare.normalise(json.loads((HERE / "expected" / "structure.json").read_text()))


@unittest.skipUnless(hasattr(os, "fork") and hasattr(os, "killpg"), "needs POSIX process groups")
class ProcessGroupTest(unittest.TestCase):
    def launch_and_exit(self, mode):
        """Start an app whose launcher exits before it listens, leaving a sleeping child."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        Path(tmp.name, "package.json").write_text("{}")
        pid_file = Path(tmp.name, "child.pid")
        command = shlex.join([sys.executable, "-c", LAUNCHER, str(pid_file), mode])
        app = adapter.UpstreamApp(tmp.name, "http://127.0.0.1:9/v1")
        with mock.patch.dict(os.environ, {"LEARNHARNESS_START": command}):
            with self.assertRaisesRegex(adapter.AdapterError, "exited with 0 before it listened"):
                app.start()
        child = int(pid_file.read_text())
        self.addCleanup(kill_recorded, child)
        self.assertTrue(process_alive(child), "the child outlived its launcher")
        self.assertEqual(os.getpgid(child), app._pgid, "the child is in the recorded group")
        return app, child

    def test_stop_ends_the_children_of_a_launcher_that_already_exited(self):
        app, child = self.launch_and_exit("sleep")
        home = app._home.name
        app.stop()
        self.assertTrue(wait_gone(child), "the sleeping child was left running")
        self.assertFalse(os.path.exists(home), "the temp HOME is removed")

    def test_a_child_that_ignores_sigterm_is_killed_after_the_grace(self):
        app, child = self.launch_and_exit("stubborn")
        started = time.monotonic()
        with mock.patch.object(adapter, "STOP_GRACE_SECONDS", 0.3):
            app.stop()
        self.assertTrue(wait_gone(child), "SIGKILL followed the grace")
        self.assertLess(time.monotonic() - started, 10, "the wait is bounded")

    def test_a_failing_group_stop_still_removes_the_temp_home(self):
        app, child = self.launch_and_exit("sleep")
        home = app._home.name
        with mock.patch.object(adapter, "stop_group", side_effect=adapter.AdapterError("stuck")):
            with self.assertRaisesRegex(adapter.AdapterError, "stuck"):
                app.stop()
        self.assertFalse(os.path.exists(home), "cleanup ran despite the failure")
        adapter.stop_group(app._pgid, app._proc)
        self.assertTrue(wait_gone(child))


class DriveCleanupTest(unittest.TestCase):
    def test_a_failing_app_still_stops_the_model_and_keeps_both_errors(self):
        stopped = []

        class Model:
            overruns = 0

            def __init__(self, turns):
                assert turns, "the script has model turns"

            def start(self):
                return "http://127.0.0.1:9/v1"

            def stop(self):
                stopped.append(True)

        class BrokenApp:
            def __init__(self, checkout, model_url):
                self.model_url = model_url

            def start(self):
                raise adapter.AdapterError("start failed")

            def stop(self):
                raise adapter.AdapterError("stop failed")

        script = json.loads((HERE / "script.json").read_text())
        with mock.patch.object(run, "FauxModel", Model):
            with self.assertRaisesRegex(adapter.AdapterError, "stop failed") as raised:
                run.drive("/unused", script, app_class=BrokenApp)
        self.assertEqual(stopped, [True], "the model was stopped")
        self.assertIn("start failed", str(raised.exception.__context__), "the first failure stays visible")


if __name__ == "__main__":
    unittest.main()
