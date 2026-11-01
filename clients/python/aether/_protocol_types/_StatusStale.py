# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class StatusStale(TypedDict, total=False):
    """
    Why a [`DaemonStatus`] is not fresh, and how old it is.
    """
    read_at: str | None  # optional
    waited_ms: int
