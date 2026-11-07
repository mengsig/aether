# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SpendToday(TypedDict, total=False):
    """
    What Noesis spent today, split by who asked for the work.
    """
    api: _protocol.Usage  # optional
    background: _protocol.Usage
    interactive: _protocol.Usage
    subscription: _protocol.Usage  # optional
