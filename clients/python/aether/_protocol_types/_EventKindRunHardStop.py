# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindRunHardStop(TypedDict, total=False):
    """
    A run crossed its hard spend stop: every live agent in it was
    interrupted (not failed) and no new turn is admitted until the limit
    is raised.
    """
    interrupted: int
    limit: str
    reason: str
    type: Literal["run.hard_stop"]
    usage: _protocol.Usage
