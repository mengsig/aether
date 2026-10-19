# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NotificationAnswer(TypedDict, total=False):
    """
    The operator's answer: a chosen action, their own words, or both.
    """
    action: str | None  # optional
    at: str
    by: _protocol.Actor
    text: str | None  # optional
