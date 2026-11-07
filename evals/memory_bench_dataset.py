"""The memory benchmark's dataset: loading, validation, and the plan the driver runs.

A set is a directory with `manifest.json` and one `scenarios/<id>.json` per
scenario. Keys are scenario-local; the plan and the metrics name a memory
`<scenario>/<key>`, and a query may label another scenario's memory that way.
"""
import json
import re
from pathlib import Path

CLASSES = ("durable", "near_duplicate", "one_off", "stale", "incorrect", "transient")
# Memories that mislead a task when they reach it.
HARMFUL_CLASSES = ("stale", "incorrect")
KINDS = ("fact", "decision", "lesson", "constraint", "preference")
SCOPE = re.compile(r"^(repository|global|role:[a-z][a-z0-9_.-]*|repo-role:[a-z][a-z0-9_.-]*)$")


class DatasetError(ValueError):
    pass


def load_set(directory):
    """The manifest and its scenarios, validated, in manifest order."""
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text())
    for field in ("version", "scenarios", "k", "limit", "memory_max"):
        if field not in manifest:
            raise DatasetError(f"manifest.json lacks {field}")
    if not manifest["scenarios"] or len(set(manifest["scenarios"])) != len(manifest["scenarios"]):
        raise DatasetError("manifest scenarios must be a non-empty list of unique ids")
    if not all(0 < k <= manifest["limit"] for k in manifest["k"]):
        raise DatasetError("every k must be between 1 and the retrieval limit")
    scenarios = []
    for scenario_id in manifest["scenarios"]:
        scenario = json.loads((directory / "scenarios" / f"{scenario_id}.json").read_text())
        if scenario.get("id") != scenario_id:
            raise DatasetError(f"scenarios/{scenario_id}.json names itself {scenario.get('id')!r}")
        scenarios.append(scenario)
    dataset = {"manifest": manifest, "scenarios": scenarios}
    validate(dataset)
    return dataset


def validate(dataset):
    memories = {}
    for scenario in dataset["scenarios"]:
        memories.update(_validate_memories(scenario))
    for scenario in dataset["scenarios"]:
        _validate_queries(scenario, memories)
    assert memories or not any(s["memories"] for s in dataset["scenarios"])
    return memories


def _validate_memories(scenario):
    seen = {}
    for memory in scenario["memories"]:
        key = qualify(scenario["id"], memory["key"])
        where = f"memory {key}"
        if key in seen:
            raise DatasetError(f"{where} appears twice")
        if memory.get("class") not in CLASSES:
            raise DatasetError(f"{where}: class must be one of {', '.join(CLASSES)}")
        if memory.get("kind") not in KINDS or not SCOPE.match(memory.get("scope", "")):
            raise DatasetError(f"{where}: bad kind or scope")
        if not all(0.0 <= memory.get(f, -1) <= 1.0 for f in ("importance", "confidence")):
            raise DatasetError(f"{where}: importance and confidence must be in 0..1")
        if (memory["class"] == "near_duplicate") != ("duplicate_of" in memory):
            raise DatasetError(f"{where}: duplicate_of goes with, and only with, near_duplicate")
        for relation in ("duplicate_of", "supersedes", "contradicts"):
            target = memory.get(relation)
            if target is not None and qualify(scenario["id"], target) not in seen:
                raise DatasetError(f"{where}: {relation} {target} must name an earlier memory")
        seen[key] = memory
    return seen


def _validate_queries(scenario, memories):
    ids = set()
    for query in scenario["queries"]:
        where = f"query {scenario['id']}/{query['id']}"
        if query["id"] in ids or not query["query"].strip():
            raise DatasetError(f"{where}: ids must be unique and queries non-empty")
        ids.add(query["id"])
        relevant = {qualify(scenario["id"], k) for k in query["relevant"]}
        harmful = {qualify(scenario["id"], k) for k in query["harmful"]}
        if relevant & harmful:
            raise DatasetError(f"{where}: a memory cannot be both relevant and harmful")
        for key in relevant | harmful:
            if key not in memories:
                raise DatasetError(f"{where}: labels unknown memory {key}")
        for key in relevant:
            if memories[key]["class"] not in ("durable",):
                raise DatasetError(f"{where}: relevant {key} must be a durable memory, a cluster's first wording")
        task = query.get("task")
        if task is not None and not (task.get("question") and task.get("answer")):
            raise DatasetError(f"{where}: a task needs a question and an answer pattern")


def qualify(scenario_id, key):
    return key if "/" in key else f"{scenario_id}/{key}"


def memory_index(dataset):
    """Every memory by qualified key, with its scenario id added."""
    index = {}
    for scenario in dataset["scenarios"]:
        for memory in scenario["memories"]:
            index[qualify(scenario["id"], memory["key"])] = dict(memory, scenario=scenario["id"])
    return index


def cluster_roots(dataset):
    """Each memory's cluster: the durable wording near-duplicates restate."""
    roots = {}
    for key, memory in memory_index(dataset).items():
        target = memory.get("duplicate_of")
        roots[key] = qualify(memory["scenario"], target) if target else key
    assert all(roots[root] == root for root in roots.values()), "a duplicate of a duplicate"
    return roots


def replacements(dataset):
    """(old, new) for each memory a later one supersedes or contradicts."""
    pairs = []
    for key, memory in memory_index(dataset).items():
        for relation in ("supersedes", "contradicts"):
            if memory.get(relation):
                pairs.append((qualify(memory["scenario"], memory[relation]), key, relation))
    return pairs


def queries(dataset):
    """Each query with qualified id and labels and its scenario's repository."""
    out = []
    for scenario in dataset["scenarios"]:
        for query in scenario["queries"]:
            out.append(dict(
                query,
                id=f"{scenario['id']}/{query['id']}",
                scenario=scenario["id"],
                repository=scenario["repository"],
                relevant=[qualify(scenario["id"], k) for k in query["relevant"]],
                harmful=[qualify(scenario["id"], k) for k in query["harmful"]],
            ))
    return out


def _scope(scope, repository):
    if scope == "repository":
        return {"scope": "repository", "id": repository}
    if scope == "global":
        return {"scope": "global"}
    name, role = scope.split(":", 1)
    if name == "role":
        return {"scope": "role", "role": role}
    assert name == "repo-role", scope
    return {"scope": "repository_role", "id": repository, "role": role}


def build_plan(dataset, *, index="none", reps=30, filler=5000):
    """The driver's input: candidates in admission order, then the queries."""
    manifest = dataset["manifest"]
    memories = []
    for scenario in dataset["scenarios"]:
        for memory in scenario["memories"]:
            entry = {
                "key": qualify(scenario["id"], memory["key"]),
                "scope": _scope(memory["scope"], scenario["repository"]),
                "kind": memory["kind"],
                "text": memory["text"],
                "importance": memory["importance"],
                "confidence": memory["confidence"],
                "tags": memory.get("tags", []),
                "age_days": float(memory.get("age_days", 0)),
                "operator": bool(memory.get("operator", False)),
                "feedback": memory.get("feedback", {}),
            }
            if memory.get("supersedes"):
                entry["supersedes"] = qualify(scenario["id"], memory["supersedes"])
            memories.append(entry)
    plan_queries = [
        {"id": q["id"], "repository": q["repository"], "role": q["role"], "query": q["query"],
         "limit": manifest["limit"]}
        for q in queries(dataset)
    ]
    assert len({m["key"] for m in memories}) == len(memories)
    return {
        "index": index,
        "memory_max": manifest["memory_max"],
        "memories": memories,
        "queries": plan_queries,
        "latency": {"reps": reps, "filler": filler},
    }
