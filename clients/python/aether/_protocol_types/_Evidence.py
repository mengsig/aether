# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Evidence(TypedDict, total=False):
    """
    One append-only row of the learner's history.
    """
    at: str
    concept: str
    confidence: int | None  # optional
    hint_rung: int | None  # optional
    id: int
    kind: _protocol.EvidenceKind
    miss_class: _protocol.MissClass | None  # optional
    mode: _protocol.ProblemMode | None  # optional
    note: str | None  # optional
    outcome: _protocol.EvidenceOutcome
    session: str | None  # optional
