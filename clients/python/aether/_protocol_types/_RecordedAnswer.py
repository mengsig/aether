# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RecordedAnswer(TypedDict, total=False):
    """
    The learner's answer as recorded.
    """
    answered_at: str
    chosen: list[str]  # optional
    confidence: int | None  # optional
    dont_know: bool
    text: str | None  # optional
    thinking: str | None  # optional
