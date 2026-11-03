"""The optional task-success arm: does a retrieved memory help a model answer?

Off by default. Each labelled task is asked three times, with no memory, with
the layer the current system retrieved, and with only the relevant memories
(the oracle). The dataset is invented, so a correct answer without memory is
a guess; the paired difference on the same tasks is the evidence. Success
split by whether retrieval surfaced a relevant memory is reported apart, as
correlation only.
"""
import re
import shlex
import subprocess

CONDITIONS = ("none", "retrieved", "oracle")
CALL_TIMEOUT_SECONDS = 180


def prompt(task, repository, layer):
    context = layer if layer else "(no memory retrieved)"
    return (
        f"You are an engineer working in the repository {repository}.\n\n"
        f"{context}\n\n"
        f"Question: {task['question']}\n"
        "Answer from the memory above when it applies; if it does not tell you, say you do not know. "
        "Reply with one short line."
    )


def oracle_layer(query, memories):
    lines = [f"- {memories[key]['text']}" for key in query["relevant"]]
    return "Memory:\n" + "\n".join(lines) if lines else ""


def plan_calls(labelled, rows, memories):
    """(query, condition, prompt) for every task, in a fixed order."""
    calls = []
    for query in labelled:
        task = query.get("task")
        if not task:
            continue
        layers = {
            "none": "",
            "retrieved": rows[query["id"]]["layer"] or "",
            "oracle": oracle_layer(query, memories),
        }
        for condition in CONDITIONS:
            calls.append((query, condition, prompt(task, query["repository"], layers[condition])))
    assert len(calls) % len(CONDITIONS) == 0
    return calls


def ask(command, text):
    """One model call through `command`, prompt on stdin, answer on stdout."""
    completed = subprocess.run(
        shlex.split(command), input=text, capture_output=True, text=True, timeout=CALL_TIMEOUT_SECONDS
    )
    if completed.returncode != 0:
        raise RuntimeError(f"task command exited {completed.returncode}: {completed.stderr.strip()[:300]}")
    return completed.stdout.strip()


def run(command, labelled, rows, memories, scored, max_calls):
    calls = plan_calls(labelled, rows, memories)
    if not calls:
        raise ValueError("no query in the set carries a task")
    if len(calls) > max_calls:
        raise ValueError(f"the task arm needs {len(calls)} calls; --task-max-calls allows {max_calls}")
    outcomes = []
    for query, condition, text in calls:
        answer = ask(command, text)
        outcomes.append({
            "query": query["id"],
            "condition": condition,
            "answer": answer[:500],
            "success": bool(re.search(query["task"]["answer"], answer)),
        })
    return summarise(command, outcomes, scored)


def summarise(command, outcomes, scored):
    by = {(o["query"], o["condition"]): o["success"] for o in outcomes}
    tasks = sorted({o["query"] for o in outcomes})
    rate = {c: sum(by[(t, c)] for t in tasks) / len(tasks) for c in CONDITIONS}
    surfaced = {q["id"]: (q["reciprocal_rank"] or 0) > 0 for q in scored}
    with_relevant = [by[(t, "retrieved")] for t in tasks if surfaced.get(t)]
    without = [by[(t, "retrieved")] for t in tasks if not surfaced.get(t)]
    assert set(by) == {(t, c) for t in tasks for c in CONDITIONS}
    return {
        "command": command,
        "tasks": len(tasks),
        "calls": len(outcomes),
        "success": rate,
        "effect_retrieved": round(rate["retrieved"] - rate["none"], 4),
        "effect_oracle": round(rate["oracle"] - rate["none"], 4),
        "success_when_relevant_retrieved": sum(with_relevant) / len(with_relevant) if with_relevant else None,
        "success_when_not": sum(without) / len(without) if without else None,
        "outcomes": outcomes,
    }
