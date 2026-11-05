#!/usr/bin/env python3
"""Print a blank scoring sheet for exported Noesis transcripts. No network, no model calls.

Each transcript is a JSON file: {"id": str, "mode": str, "messages": [{"role": str, "phase": str?,
"text": str}]}. A human, or a subscription model you run yourself, fills the sheet in.
"""
import argparse
import json
import sys
from pathlib import Path

RUBRIC = Path(__file__).resolve().parent / "rubric.json"
MAX_TEXT = 600


class TranscriptError(Exception):
    pass


def load_transcript(path):
    data = json.loads(Path(path).read_text())
    messages = data.get("messages") if isinstance(data, dict) else None
    if not isinstance(messages, list) or not messages:
        raise TranscriptError(f"{path}: expected an object with a non-empty 'messages' list")
    for number, message in enumerate(messages, 1):
        if not isinstance(message.get("role"), str) or not isinstance(message.get("text"), str):
            raise TranscriptError(f"{path}: message {number} needs string 'role' and 'text'")
    return {"id": data.get("id") or Path(path).stem, "mode": data.get("mode", "?"), "messages": messages}


def render_transcript(transcript):
    lines = []
    for message in transcript["messages"]:
        phase = f"/{message['phase']}" if message.get("phase") else ""
        text = " ".join(message["text"].split())
        lines.append(f"  [{message['role']}{phase}] {text[:MAX_TEXT]}" + ("..." if len(text) > MAX_TEXT else ""))
    return lines


def render_sheet(rubric, transcripts):
    assert rubric["dimensions"], "the rubric has no dimensions"
    assert transcripts, "no transcripts to score"
    scale = rubric["scale"]
    out = [f"Score each criterion {scale['min']}-{scale['max']} ({scale['meaning']}). Note evidence for low scores."]
    for transcript in transcripts:
        out += ["", f"=== transcript {transcript['id']} (mode: {transcript['mode']}) ===", *render_transcript(transcript)]
        for dimension in rubric["dimensions"]:
            out += ["", f"{dimension['title']} ({dimension['applies_to']})"]
            out += [f"  [ ] /{scale['max']}  {criterion}" for criterion in dimension["criteria"]]
        out += ["", "Notes:"]
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcripts", nargs="+", help="exported transcript JSON files")
    args = parser.parse_args(argv)
    try:
        transcripts = [load_transcript(p) for p in args.transcripts]
    except (OSError, json.JSONDecodeError, TranscriptError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(render_sheet(json.loads(RUBRIC.read_text()), transcripts))
    return 0


if __name__ == "__main__":
    sys.exit(main())
