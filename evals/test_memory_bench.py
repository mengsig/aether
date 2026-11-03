"""Tests of the memory benchmark's dataset, metrics, baseline check and task arm.

They run without cargo or a model: the driver's report is built by hand.
"""
import copy
import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import memory_bench
import memory_bench_task as task_arm
from memory_bench_dataset import DatasetError, build_plan, load_set, validate
from memory_bench_metrics import compute, differences, percentile
from memory_bench_report import compare, render

SHIPPED_SET = Path(__file__).resolve().parent.parent / "bench" / "memory-set" / "v1"


def memory(key, cls="durable", **extra):
    return dict({"key": key, "class": cls, "scope": "repository", "kind": "fact",
                 "importance": 0.6, "confidence": 0.8, "text": f"text of {key}"}, **extra)


def tiny_dataset():
    return {
        "manifest": {"name": "tiny", "version": "v0", "scenarios": ["s"], "k": [1, 2], "limit": 3,
                     "memory_max": 2000},
        "scenarios": [{
            "id": "s",
            "repository": "github.com/example-org/s",
            "memories": [
                memory("rule"),
                memory("rule-again", "near_duplicate", duplicate_of="rule"),
                memory("old"),
                memory("new", supersedes="old"),
                memory("stale", "stale"),
                memory("fresh", contradicts="stale"),
                memory("once", "one_off"),
                memory("passing", "transient"),
            ],
            "queries": [
                {"id": "q1", "role": "worker", "query": "use the rule", "relevant": ["rule"], "harmful": []},
                {"id": "q2", "role": "worker", "query": "what is fresh", "relevant": ["fresh"],
                 "harmful": ["stale"], "task": {"question": "What is fresh?", "answer": "text of fresh"}},
                {"id": "q3", "role": "worker", "query": "nothing", "relevant": [], "harmful": []},
            ],
        }],
    }


def ranked(*keys, tokens=10):
    return [{"key": f"s/{k}", "score": 1.0, "tokens": tokens, "injected": True} for k in keys]


def tiny_report():
    outcomes = {"rule-again": "accepted", "new": "superseded", "passing": "rejected"}
    keys = ["rule", "rule-again", "old", "new", "stale", "fresh", "once", "passing"]
    return {
        "index": "none",
        "admissions": [{"key": f"s/{k}", "outcome": outcomes.get(k, "accepted")} for k in keys],
        "current": [f"s/{k}" for k in ["fresh", "new", "once", "rule", "rule-again", "stale"]],
        "queries": [
            {"id": "s/q1", "ranked": ranked("rule", "rule-again", "once"), "layer_tokens": 40,
             "layer": "Memory:\n- text of rule"},
            {"id": "s/q2", "ranked": ranked("stale", "fresh"), "layer_tokens": 30,
             "layer": "Memory:\n- text of stale\n- text of fresh"},
            {"id": "s/q3", "ranked": ranked("once"), "layer_tokens": 20, "layer": "Memory:\n- text of once"},
        ],
        "latency": {"small": [{"id": "s/q1", "micros": [3.0, 1.0, 2.0]}],
                    "scaled": {"memories": 100, "footprint": {"database": 9000, "total": 12000},
                               "timings": [{"id": "s/q1", "micros": [5.0]}]}},
        "storage": {"empty": {"database": 1000, "total": 1500}, "seeded": {"database": 1700, "total": 4000},
                    "records": 7, "current": 6},
        "restart": {"queries": 3, "identical": 3, "current_before": 6, "current_after": 6},
        "corruption": [
            {"case": "intact_copy", "open": "ok", "retrieval": "ok", "recovered": 5, "expected": 5},
            {"case": "zeroed_header", "open": "error", "retrieval": "skipped", "recovered": None, "expected": 5},
        ],
    }


class ShippedSetTest(unittest.TestCase):
    def test_the_shipped_set_validates_and_plans_every_memory_and_query(self):
        dataset = load_set(SHIPPED_SET)
        plan = build_plan(dataset, reps=1, filler=0)
        offered = sum(len(s["memories"]) for s in dataset["scenarios"])
        self.assertEqual(len(plan["memories"]), offered)
        self.assertEqual(len(plan["queries"]), sum(len(s["queries"]) for s in dataset["scenarios"]))
        self.assertTrue(all(q["limit"] == dataset["manifest"]["limit"] for q in plan["queries"]))
        classes = {m["class"] for s in dataset["scenarios"] for m in s["memories"]}
        self.assertEqual(classes, {"durable", "near_duplicate", "one_off", "stale", "incorrect", "transient"})

    def test_the_shipped_set_names_only_placeholder_owners(self):
        for path in (SHIPPED_SET / "scenarios").glob("*.json"):
            text = path.read_text()
            self.assertNotIn("/home/", text, path)
            for scenario_repo in [json.loads(text)["repository"]]:
                self.assertTrue(scenario_repo.startswith("github.com/example-org/"), path)

    def test_the_committed_baseline_covers_both_arms(self):
        baseline = json.loads((SHIPPED_SET / "baseline.json").read_text())
        self.assertEqual(set(baseline["arms"]), set(memory_bench.ARMS))
        self.assertEqual(len(baseline["commit"]), 40)


class DatasetValidationTest(unittest.TestCase):
    def assertRefused(self, dataset, fragment):
        with self.assertRaises(DatasetError) as caught:
            validate(dataset)
        self.assertIn(fragment, str(caught.exception))

    def test_a_relevant_label_on_a_near_duplicate_is_refused(self):
        dataset = tiny_dataset()
        dataset["scenarios"][0]["queries"][0]["relevant"] = ["rule-again"]
        self.assertRefused(dataset, "must be a durable memory")

    def test_a_relation_to_a_later_memory_is_refused(self):
        dataset = tiny_dataset()
        memories = dataset["scenarios"][0]["memories"]
        memories[2], memories[3] = memories[3], memories[2]
        self.assertRefused(dataset, "supersedes old must name an earlier memory")

    def test_an_unknown_label_and_a_doubly_labelled_memory_are_refused(self):
        dataset = tiny_dataset()
        dataset["scenarios"][0]["queries"][0]["harmful"] = ["missing"]
        self.assertRefused(dataset, "labels unknown memory s/missing")
        dataset = tiny_dataset()
        dataset["scenarios"][0]["queries"][1]["harmful"] = ["fresh"]
        self.assertRefused(dataset, "both relevant and harmful")

    def test_scopes_map_to_the_core_scope_shapes(self):
        dataset = tiny_dataset()
        dataset["scenarios"][0]["memories"][0]["scope"] = "repo-role:reviewer"
        dataset["scenarios"][0]["memories"][2]["scope"] = "role:worker"
        plan = build_plan(dataset)
        scopes = [m["scope"] for m in plan["memories"]]
        self.assertEqual(scopes[0], {"scope": "repository_role", "id": "github.com/example-org/s", "role": "reviewer"})
        self.assertEqual(scopes[2], {"scope": "role", "role": "worker"})
        self.assertEqual(scopes[1], {"scope": "repository", "id": "github.com/example-org/s"})
        self.assertEqual(plan["memories"][3]["supersedes"], "s/old")


class MetricsTest(unittest.TestCase):
    def setUp(self):
        self.metrics = compute(tiny_dataset(), tiny_report())
        self.det = self.metrics["deterministic"]

    def test_a_restated_memory_counts_as_redundant_not_relevant(self):
        q1 = self.metrics["queries"][0]
        self.assertEqual(q1["precision@2"], 0.5)
        self.assertEqual(q1["recall@1"], 1.0)
        self.assertEqual(q1["redundant_slots"], 1)
        self.assertEqual(q1["wasted_tokens"], 20)

    def test_a_query_without_relevant_memories_is_scored_on_noise_only(self):
        q3 = self.metrics["queries"][2]
        self.assertEqual(q3["precision@1"], 0.0)
        self.assertIsNone(q3["recall@1"])
        self.assertEqual(self.det["injection"]["tokens_on_queries_without_relevant"], 20)
        self.assertEqual(self.det["retrieval"]["queries_with_relevant"], 2)

    def test_a_stale_fact_ranked_above_its_replacement_is_a_lost_conflict(self):
        self.assertEqual(self.metrics["conflicts"], [
            {"query": "s/q2", "old": "s/stale", "new": "s/fresh", "relation": "contradicts", "outcome": "harmful_first"},
        ])
        self.assertEqual(self.det["influence"]["conflict_loss_rate"], 1.0)
        self.assertEqual(self.det["influence"]["harmful_exposure_rate"], 1.0)
        self.assertEqual(self.det["retrieval"]["mrr"], 0.75)

    def test_store_quality_counts_current_memories_by_class(self):
        store = self.det["store"]
        self.assertEqual(store["current"], 6)
        self.assertEqual(store["duplicate_share"], round(1 / 6, 4))
        self.assertEqual(store["low_value_share"], round(1 / 6, 4))
        self.assertEqual(store["transient_rejected"], 1.0)
        self.assertEqual(store["near_duplicates_admitted"], 1.0)

    def test_store_quality_reads_current_memories_from_the_store(self):
        report = tiny_report()
        report["current"].remove("s/rule-again")
        report["storage"]["current"] = 5
        self.assertEqual(compute(tiny_dataset(), report)["deterministic"]["store"]["duplicate_share"], 0.0)
        report = tiny_report()
        report["current"].append("s/old")
        report["storage"]["current"] = 7
        self.assertEqual(compute(tiny_dataset(), report)["deterministic"]["store"]["current"], 7)
        report["storage"]["current"] = 6
        with self.assertRaises(AssertionError):
            compute(tiny_dataset(), report)

    def test_only_injected_lines_count_as_exposure_and_slots(self):
        report = tiny_report()
        report["queries"][1]["ranked"][0]["injected"] = False
        report["queries"][1]["layer"] = "Memory:\n- text of fresh"
        partial = compute(tiny_dataset(), report)
        self.assertEqual(partial["queries"][1]["shown"], ["s/fresh"])
        self.assertEqual(partial["queries"][1]["precision@2"], 0.5, "retrieval still scores the ranking")
        self.assertEqual(partial["conflicts"][0]["outcome"], "replacement_only")
        self.assertEqual(partial["deterministic"]["influence"]["harmful_exposure_rate"], 0.0)
        for item in report["queries"][1]["ranked"]:
            item["injected"] = False
        report["queries"][1].update(layer=None, layer_tokens=0)
        empty = compute(tiny_dataset(), report)
        self.assertEqual(empty["queries"][1]["slots"], 0)
        self.assertEqual(empty["conflicts"][0]["outcome"], "neither")
        self.assertEqual(empty["deterministic"]["injection"]["mean_slots"], round(4 / 3, 4))

    def test_measured_and_recovery_sections(self):
        measured = self.metrics["measured"]
        self.assertEqual(measured["latency_us"]["median"], 2.0)
        self.assertEqual(measured["storage_bytes"]["per_record"], 100.0)
        recovery = self.det["recovery"]
        self.assertEqual(recovery["corruption"]["zeroed_header"]["recovered_share"], None)
        self.assertTrue(recovery["restart_memories_kept"])

    def test_a_report_for_other_queries_is_refused(self):
        report = tiny_report()
        report["queries"].pop()
        with self.assertRaises(AssertionError):
            compute(tiny_dataset(), report)

    def test_percentile_is_nearest_rank(self):
        self.assertEqual(percentile([], 0.5), 0.0)
        self.assertEqual(percentile([4, 1, 3, 2], 0.5), 2)
        self.assertEqual(percentile(list(range(1, 101)), 0.95), 95)


class BaselineCheckTest(unittest.TestCase):
    def results(self):
        metrics = compute(tiny_dataset(), tiny_report())
        return {"set": "tiny/v0", "commit": "a" * 40, "dirty": False, "arms": {"none": metrics}}

    def test_a_reproduced_baseline_passes_and_a_moved_metric_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "baseline.json"
            with redirect_stderr(io.StringIO()):
                memory_bench.write_baseline(path, self.results())
            with redirect_stdout(io.StringIO()):
                self.assertEqual(memory_bench.check(path, self.results()), 0)
            moved = self.results()
            moved["arms"]["none"]["deterministic"]["retrieval"]["mrr"] = 0.5
            err = io.StringIO()
            with redirect_stderr(err):
                self.assertEqual(memory_bench.check(path, moved), 1)
            self.assertIn("retrieval.mrr: expected 0.75, got 0.5", err.getvalue())

    def test_differences_reports_missing_and_changed_leaves(self):
        self.assertEqual(differences({"a": {"b": 1}}, {"a": {"b": 1}}), [])
        self.assertEqual(differences({"a": 1}, {"a": 2, "c": 3}),
                         ["a: expected 1, got 2", "c: expected None, got 3"])

    def test_report_and_comparison_render_every_row(self):
        before = self.results()
        after = copy.deepcopy(before)
        after["arms"]["none"]["deterministic"]["retrieval"]["mrr"] = 1.0
        text = render(before)
        self.assertIn("| MRR (higher is better) | 0.75 |", text)
        self.assertIn("| zeroed_header | error | skipped | - |", text)
        self.assertIn("| MRR (higher is better) | 0.75 | 1 | 0.25 |", compare(before, after))


class TaskArmTest(unittest.TestCase):
    def test_an_echoing_model_succeeds_only_where_the_answer_was_in_context(self):
        dataset = tiny_dataset()
        from memory_bench_dataset import memory_index, queries
        rows = {row["id"]: row for row in tiny_report()["queries"]}
        scored = compute(dataset, tiny_report())["queries"]
        result = task_arm.run("cat", queries(dataset), rows, memory_index(dataset), scored, max_calls=3)
        self.assertEqual(result["success"], {"none": 0.0, "retrieved": 1.0, "oracle": 1.0})
        self.assertEqual(result["effect_retrieved"], 1.0)
        self.assertEqual(result["success_when_relevant_retrieved"], 1.0)
        self.assertIsNone(result["success_when_not"])

    def test_a_task_arm_over_its_call_cap_is_refused_before_any_call(self):
        dataset = tiny_dataset()
        from memory_bench_dataset import memory_index, queries
        rows = {row["id"]: row for row in tiny_report()["queries"]}
        with self.assertRaises(ValueError) as caught:
            task_arm.run("false", queries(dataset), rows, memory_index(dataset), [], max_calls=2)
        self.assertIn("needs 3 calls", str(caught.exception))

    def test_a_failing_model_command_is_an_error_not_a_miss(self):
        with self.assertRaises(RuntimeError):
            task_arm.ask("false", "prompt")


if __name__ == "__main__":
    unittest.main()
