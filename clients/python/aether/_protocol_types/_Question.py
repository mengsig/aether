# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Question(TypedDict, total=False):
    """
    A question put to the learner. The answer key is never part of it: it
    is revealed in a [`QuestionResult`] once the answer is recorded.
    """
    collapsed: bool
    concepts: list[str]
    id: str
    kind: _protocol.QuestionKind
    multi: bool
    options: list[_protocol.QuestionOption] | None  # optional
    plan_node: str | None  # optional
    posed_at: str
    prompt: str
    step: _protocol.QuestionStep | None  # optional
