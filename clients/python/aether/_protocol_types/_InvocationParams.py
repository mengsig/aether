# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class InvocationParams(TypedDict, total=False):
    effort: _protocol.Effort | None  # optional
    effort_effective: _protocol.Effort | None  # optional
    effort_requested: _protocol.Effort | None  # optional
    extensions: dict[str, Any]  # optional
    max_output_tokens: int | None  # optional
    temperature: float | None  # optional
    thinking: _protocol.ThinkingMode | None  # optional
