# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


# What became of a keyed command, as far as the daemon can prove. Each
# carries the daemon's `horizon` now; `retained_since` is for people to read.
# Tagged by `outcome`.
class SubmissionResolutionExecuted(TypedDict, total=False):
    """
    It ran. `reply` is what a duplicate under the same key is answered.
    """
    horizon: str
    outcome: Literal["executed"]
    reply: _protocol.Reply
