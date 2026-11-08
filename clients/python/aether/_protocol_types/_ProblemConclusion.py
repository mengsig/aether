# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ProblemConclusion(TypedDict, total=False):
    """
    How a problem ended, as `noesis.practice_concluded` announced it.
    """
    concluded_at: str
    miss_class: _protocol.MissClass | None  # optional
    outcome: _protocol.PracticeOutcome
    retest: bool
    rung: int | None  # optional
    solution: str
