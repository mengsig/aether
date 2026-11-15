# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReferenceRequest(TypedDict, total=False):
    """
    One reference as a creator names it: `{path, kind, label}`.
    """
    kind: _protocol.ReferenceKind
    label: str | None  # optional
    path: str
