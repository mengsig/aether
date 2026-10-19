# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Notification(TypedDict, total=False):
    actions: list[_protocol.NotificationAction]  # optional
    answer: _protocol.NotificationAnswer | None  # optional
    body: str  # optional
    created_at: str
    free_text: bool  # optional
    id: _protocol.NotificationId
    key: str | None  # optional
    kind: _protocol.NotificationKind
    link: str | None  # optional
    source: str
    state: _protocol.NotificationState
    title: str
    updated_at: str
