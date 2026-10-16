# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyNoteAppended(TypedDict, total=False):
    """
    The note's event, by its sequence number in the log.
    """
    data: dict[str, Any]
    reply: Literal["note_appended"]
