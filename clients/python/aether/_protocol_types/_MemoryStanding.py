# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class MemoryStanding(TypedDict, total=False):
    """
    What admission and retrieval know about a record, sent beside it to a
    client that lists `memory_v2`. Never part of the stored body.
    """
    refs: list[_protocol.MemoryRef]  # optional
    stale: _protocol.StaleNote | None  # optional
    state: _protocol.MemoryState
    support: _protocol.MemorySupport
    tier: _protocol.MemoryTier
    verification: _protocol.Verification
