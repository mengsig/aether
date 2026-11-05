"""Tests of the differential harness that need no checkout: skipping, comparator, faux model."""
import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent / "differential"
sys.path.insert(0, str(HERE))
import adapter
import compare
from faux_model import FauxModel, sse_chunks

RUN = HERE / "run.py"


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


if __name__ == "__main__":
    unittest.main()
