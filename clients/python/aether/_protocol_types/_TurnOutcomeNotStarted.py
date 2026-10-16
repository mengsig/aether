# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class TurnOutcomeNotStarted(TypedDict, total=False):
    """
    No turn has started: a turn-0 frame holding the notes written so far.
    """
    outcome: Literal["not_started"]
