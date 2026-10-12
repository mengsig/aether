# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._ServerFrameChallenge import ServerFrameChallenge
from ._ServerFrameDelta import ServerFrameDelta
from ._ServerFrameEvent import ServerFrameEvent
from ._ServerFrameReply import ServerFrameReply


ServerFrame = ServerFrameReply | ServerFrameEvent | ServerFrameDelta | ServerFrameChallenge
