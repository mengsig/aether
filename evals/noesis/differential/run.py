#!/usr/bin/env python3
"""Opt-in differential harness: drive the user's own LearnHarness checkout as a black box.

Never run by CI. Without a checkout it prints SKIP and exits 0. See ../README.md.
"""
import argparse
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import adapter
import compare
from faux_model import FauxModel

EXPECTED = HERE / "expected" / "structure.json"
SCRIPT = HERE / "script.json"


def skip_reason(path):
    if not path:
        return "no checkout given (pass --learnharness <path> or set LEARNHARNESS_DIR)"
    if not adapter.looks_like_checkout(path):
        return f"{path} is not a checkout (no package.json, pyproject.toml or requirements.txt)"
    return None


def drive(checkout, script):
    """Start the faux model and the upstream app, run the script, return the upstream trace."""
    model = FauxModel(script["model_turns"])
    app = adapter.UpstreamApp(checkout, model.start())
    try:
        app.start()
        app.configure()
        state = app.converse(script["topic"], script["user_messages"])
    finally:
        app.stop()
        model.stop()
    if model.overruns:
        raise adapter.AdapterError(f"the app made {model.overruns} model calls past the script")
    return adapter.trace_from_state(state)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--learnharness", default=os.environ.get("LEARNHARNESS_DIR", ""))
    parser.add_argument("--trace-out", help="write the upstream structural trace here")
    args = parser.parse_args(argv)
    reason = skip_reason(args.learnharness)
    if reason:
        print(f"SKIP: {reason}")
        return 0
    trace = drive(args.learnharness, json.loads(SCRIPT.read_text()))
    if args.trace_out:
        Path(args.trace_out).write_text(json.dumps(trace, indent=2) + "\n")
    found = compare.differences(json.loads(EXPECTED.read_text()), trace)
    for line in found:
        print(f"DIFF {line}")
    print("MATCH: structure equals the Noesis expectation" if not found else f"{len(found)} structural difference(s)")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
