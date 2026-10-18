# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplayNote(TypedDict, total=False):
    """
    A note appended to the transcript; it started no turn.
    """
    at: str
    context: _protocol.NoteContext  # optional
    level: _protocol.NoteLevel  # optional
    links: list[str]  # optional
    seq: int
    text: str
