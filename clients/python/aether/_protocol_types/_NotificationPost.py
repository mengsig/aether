# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NotificationPost(TypedDict, total=False):
    """
    What a poster sends. Posting the same `key` again from the same source
    updates that notification instead of adding another.
    """
    actions: list[_protocol.NotificationAction]  # optional
    body: str  # optional
    free_text: bool  # optional
    key: str | None  # optional
    kind: _protocol.NotificationKind
    link: str | None  # optional
    title: str
