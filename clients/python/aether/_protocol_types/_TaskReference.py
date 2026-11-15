# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class TaskReference(TypedDict, total=False):
    """
    One checked reference, as the task stores it.
    """
    image: _protocol.AttachmentRef | None  # optional
    kind: _protocol.ReferenceKind
    label: str
    path: str
    sha256: str
    size_bytes: int
