# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindCompactionNative(TypedDict, total=False):
    """
    The engine or provider serving the agent folded the context itself
    (Claude Code's `/compact`, Anthropic's server-side summary). No
    checkpoint exists: the summary lives in the engine session or the transcript.
    """
    after_tokens: int | None  # optional
    automatic: bool  # optional
    before_tokens: int | None  # optional
    duration_ms: int | None  # optional
    provider: str
    type: Literal["compaction.native"]
