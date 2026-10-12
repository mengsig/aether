# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class AttachmentRef(TypedDict, total=False):
    """
    A stored image, as inboxes, events and transcripts carry it.
    """
    id: _protocol.AttachmentId
    media_type: str
    name: str | None  # optional
    size_bytes: int
