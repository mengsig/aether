# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What a question does in the lesson: a probe measures, a teach-phase
# question predicts before a step or checks after it.
QuestionStep = Literal["measure", "predict", "check", "goal", "practice"]
