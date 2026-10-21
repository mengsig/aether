# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class CapabilityProfile(TypedDict, total=False):
    """
    What a model can do. Declared per model in the registry so callers can ask
    before they depend on a feature.
    """
    compaction_tokens: int | None  # optional
    context_window: int
    cost: _protocol.ModelCost | None  # optional
    effort: bool  # optional
    extensions: dict[str, Any]  # optional
    max_output_tokens: int
    parallel_tools: bool  # optional
    prompt_caching: bool  # optional
    strict_tools: bool  # optional
    structured_output: bool  # optional
    thinking: _protocol.ThinkingSupport  # optional
    tools: bool  # optional
    vision: bool  # optional
