# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._TurnOutcomeCompleted import TurnOutcomeCompleted
from ._TurnOutcomeFailed import TurnOutcomeFailed
from ._TurnOutcomeInterrupted import TurnOutcomeInterrupted
from ._TurnOutcomeNotStarted import TurnOutcomeNotStarted
from ._TurnOutcomeRunning import TurnOutcomeRunning


TurnOutcome = (
    TurnOutcomeRunning |
    TurnOutcomeCompleted |
    TurnOutcomeFailed |
    TurnOutcomeInterrupted |
    TurnOutcomeNotStarted
)
