# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WorkspaceDiff(TypedDict, total=False):
    """
    An agent's work in its workspace, bounded for the wire.
    """
    agent: _protocol.AgentId
    ambiguous: bool
    commits: list[_protocol.WorkspaceCommit]
    commits_truncated: bool
    deletions: int
    files: list[_protocol.ChangedFile]
    files_truncated: bool
    insertions: int
    integrated: bool
    isolated: bool
    patch: str
    patch_bytes: int
    patch_truncated: bool
    source: str | None  # optional
    workspace: str
