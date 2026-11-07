# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EvidenceBundle(TypedDict, total=False):
    """
    What the frontend gathered from reads the user can already make, stored
    with the session that starts from it.
    """
    items: list[_protocol.EvidenceItem]
    title: str
