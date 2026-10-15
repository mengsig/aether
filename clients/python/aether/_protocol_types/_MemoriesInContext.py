# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoriesInContext(TypedDict, total=False):
    """
    Which memories each model call of a task, or one invocation, was given,
    oldest snapshot first. `truncated` says snapshots past the limit were left out.
    """
    snapshots: list[_protocol.SnapshotMemories]
    truncated: bool  # optional
