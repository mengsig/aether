# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NotificationAction(TypedDict, total=False):
    """
    A choice the operator can make; its `id` is what the poster receives.
    """
    id: str
    label: str
