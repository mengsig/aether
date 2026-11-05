# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    MemoryScope,
)

def consolidate_memories(*, dry_run: bool, scope: MemoryScope | None = None) -> dict[str, Any]:
    """Merge near-duplicate memories, or with `dry_run` only plan it.
    An absent scope means every scope. Needs `memory_v2`.
    """
    frame: dict[str, Any] = {"cmd": "consolidate_memories"}
    frame["dry_run"] = dry_run
    if scope is not None:
        frame["scope"] = scope
    return frame
