# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisProvenance(TypedDict, total=False):
    """
    Where a proposition or chain came from.
    """
    message_ref: str | None  # optional
    source: str
