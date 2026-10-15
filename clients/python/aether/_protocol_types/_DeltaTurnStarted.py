# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


# Transient streaming signal. Not persisted; durable events reconcile it.
# Tagged by `delta`.
class DeltaTurnStarted(TypedDict, total=False):
    delta: Literal["turn_started"]
    turn: int
