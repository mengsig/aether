# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ConceptState(TypedDict, total=False):
    as_of_evidence: int
    calibration: float | None  # optional
    concept: _protocol.ConceptRef
    last_success_at: str | None  # optional
    misconceptions: list[str]
    review_due_at: str | None  # optional
    state: _protocol.ConceptStateKind
    strength: float
