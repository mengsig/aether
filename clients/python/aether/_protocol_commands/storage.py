# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def storage_gc(*, dry_run: bool, prune_build_output: bool | None = None) -> dict[str, Any]:
    """Host-only filesystem maintenance. Preview never deletes files."""
    frame: dict[str, Any] = {"cmd": "storage_gc"}
    frame["dry_run"] = dry_run
    if prune_build_output is not None:
        frame["prune_build_output"] = prune_build_output
    return frame

def storage_status() -> dict[str, Any]:
    """Bounded filesystem inventory and eligibility preview."""
    frame: dict[str, Any] = {"cmd": "storage_status"}
    return frame
