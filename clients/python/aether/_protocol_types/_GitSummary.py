# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class GitSummary(TypedDict, total=False):
    ahead: int | None  # optional
    behind: int | None  # optional
    branch: str | None  # optional
    detached: bool  # optional
    dirty: bool  # optional
    head: str | None  # optional
    modified: int  # optional
    repository: bool
    root: str | None  # optional
    staged: int  # optional
    untracked: int  # optional
