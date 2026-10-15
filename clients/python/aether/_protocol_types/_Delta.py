# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._DeltaAccounting import DeltaAccounting
from ._DeltaText import DeltaText
from ._DeltaThinking import DeltaThinking
from ._DeltaToolInput import DeltaToolInput
from ._DeltaTurnFinished import DeltaTurnFinished
from ._DeltaTurnStarted import DeltaTurnStarted


Delta = (
    DeltaTurnStarted |
    DeltaText |
    DeltaThinking |
    DeltaToolInput |
    DeltaAccounting |
    DeltaTurnFinished
)
