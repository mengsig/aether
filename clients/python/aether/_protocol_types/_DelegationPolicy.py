# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DelegationPolicy(TypedDict, total=False):
    """
    The children a task's agent may create. Every field is optional; an empty
    policy bounds nothing beyond the rules for a task without one.
    """
    default_tier: _protocol.ModelTier | None  # optional
    exempt_models: list[_protocol.ModelRef]  # optional
    exempt_roles: list[str]  # optional
    max_effort: _protocol.Effort | None  # optional
    max_parallel: int | None  # optional
    require_independent_vendor: bool  # optional
    workers: list[_protocol.WorkerChoice]  # optional
