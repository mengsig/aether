# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SessionUsage(TypedDict, total=False):
    """
    A session's usage by billing: subscription use is covered by the
    learner's plan and never charged per request; API use is.
    """
    api: _protocol.Usage
    subscription: _protocol.Usage
    unknown: _protocol.Usage  # optional
