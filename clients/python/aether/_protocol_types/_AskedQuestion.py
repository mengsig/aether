# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class AskedQuestion(TypedDict, total=False):
    """
    A posed question with what the learner answered and what it revealed.
    """
    answer: _protocol.RecordedAnswer | None  # optional
    question: _protocol.Question
    result: _protocol.QuestionResult | None  # optional
