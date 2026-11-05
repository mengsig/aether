"""Structure-only comparison of two differential traces: never ids, never bytes."""
import re

TRACE_KEYS = ("phases", "questions", "plan_validations", "graph")


class TraceError(Exception):
    pass


def normalise_statement(text):
    assert isinstance(text, str), "a statement is text"
    return re.sub(r"\s+", " ", text.strip().lower())


def normalise(trace):
    """Return a canonical copy: statements folded, sets sorted, ordered lists kept."""
    missing = [k for k in TRACE_KEYS if k not in trace]
    if missing:
        raise TraceError(f"trace lacks {missing}")
    graph = trace["graph"]
    premise_sets = [
        {
            "conclusion": normalise_statement(item["conclusion"]),
            "premises": sorted(normalise_statement(p) for p in item["premises"]),
        }
        for item in graph["premise_sets"]
    ]
    return {
        "phases": list(trace["phases"]),
        "questions": [{"valid": bool(q["valid"])} for q in trace["questions"]],
        "plan_validations": [
            {"valid": bool(v["valid"]), "problems": sorted(v["problems"])} for v in trace["plan_validations"]
        ],
        "graph": {
            "statements": sorted({normalise_statement(s) for s in graph["statements"]}),
            "premise_sets": sorted(premise_sets, key=lambda i: (i["conclusion"], i["premises"])),
        },
    }


def differences(expected, actual):
    """Return human-readable differences between two traces; empty means they match."""
    want, got = normalise(expected), normalise(actual)
    found = []
    for key in TRACE_KEYS:
        if want[key] != got[key]:
            found.append(f"{key}: expected {want[key]!r}, got {got[key]!r}")
    return found
