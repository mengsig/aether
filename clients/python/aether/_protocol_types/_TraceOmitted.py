# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class TraceOmitted(TypedDict, total=False):
    """
    Candidates past the trace's caps, by the outcome they had.
    """
    cut: int
    over_limit: int
