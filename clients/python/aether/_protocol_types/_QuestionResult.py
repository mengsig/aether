# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class QuestionResult(TypedDict, total=False):
    answer_key: _protocol.AnswerKey | None  # optional
    correct: bool | None  # optional
    explanation: str | None  # optional
    question: str
