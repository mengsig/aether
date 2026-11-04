# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DisablePolicy(TypedDict, total=False):
    """
    What a disable does to work in flight.
    """
    background: _protocol.BackgroundDisable
    grace_seconds: int
