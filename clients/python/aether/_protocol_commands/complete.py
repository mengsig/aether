# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    PathCompletionKind,
)

def complete_path(*, kind: PathCompletionKind, prefix: str, limit: int | None = None, workspace: str | None = None) -> dict[str, Any]:
    """Complete a partial path on the daemon's machine. Bounded: at most
    `limit` entries (default 50, never more than 200).
    """
    frame: dict[str, Any] = {"cmd": "complete_path"}
    frame["kind"] = kind
    if limit is not None:
        frame["limit"] = limit
    frame["prefix"] = prefix
    if workspace is not None:
        frame["workspace"] = workspace
    return frame
