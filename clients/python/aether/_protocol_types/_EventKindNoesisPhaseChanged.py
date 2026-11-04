# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindNoesisPhaseChanged(TypedDict, total=False):
    from_: _protocol.Phase
    session: str
    to: _protocol.Phase
    type: Literal["noesis.phase_changed"]
