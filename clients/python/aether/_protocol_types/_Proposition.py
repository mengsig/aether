# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Proposition(TypedDict, total=False):
    aliases: list[str]
    created_at: str
    id: str
    name: str
    provenance: list[_protocol.NoesisProvenance]
    statement: str
