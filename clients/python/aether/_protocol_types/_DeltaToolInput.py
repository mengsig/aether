# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DeltaToolInput(TypedDict, total=False):
    delta: Literal["tool_input"]
    name: str
    partial_json: str
    tool_use_id: str | None  # optional
