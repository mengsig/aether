# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindNoesisPracticeConcluded(TypedDict, total=False):
    """
    The only place a problem's sealed solution is revealed.
    """
    miss_class: _protocol.MissClass | None  # optional
    outcome: _protocol.PracticeOutcome
    problem: str
    retest: bool
    rung: int | None  # optional
    session: str
    solution: str
    type: Literal["noesis.practice_concluded"]
