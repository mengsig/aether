# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RetrievalTrace(TypedDict, total=False):
    """
    What retrieval did for one turn, recorded with the snapshot it built.
    """
    candidates: list[_protocol.TraceCandidate]
    omitted: _protocol.TraceOmitted  # optional
    policy: _protocol.TracePolicy
    pool: int
