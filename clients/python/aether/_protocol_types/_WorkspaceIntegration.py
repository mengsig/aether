# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WorkspaceIntegration(TypedDict, total=False):
    """
    The result of an operator's integration. Refused with `conflicts` when
    the source changed a path the child also changed; nothing was written.
    """
    agent: _protocol.AgentId
    already_integrated: bool
    changed_paths: list[str]
    conflicts: list[str]
    integrated: bool
    source: str
