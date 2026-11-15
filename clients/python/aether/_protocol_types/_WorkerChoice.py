# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class WorkerChoice(TypedDict, total=False):
    """
    One allowed kind of child: an exact model or a whole tier, optionally
    capped in effort. Exactly one of `model` and `tier` is set.
    """
    effort: _protocol.Effort | None  # optional
    model: _protocol.ModelRef | None  # optional
    tier: _protocol.ModelTier | None  # optional
