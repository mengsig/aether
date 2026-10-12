# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    AttentionKind,
    EventQuery,
    PushSubscription,
)

def subscribe(*, deltas: bool, query: EventQuery, after: int | None = None) -> dict[str, Any]:
    """Stream events with `seq > after` (0 replays everything, `None` starts live)."""
    frame: dict[str, Any] = {"cmd": "subscribe"}
    if after is not None:
        frame["after"] = after
    frame["deltas"] = deltas
    frame["query"] = query
    return frame

def subscribe_push(*, subscription: PushSubscription, kinds: list[AttentionKind] | None = None) -> dict[str, Any]:
    """Store the calling device's push subscription, replacing any earlier
    one. Only a paired device can call it.
    """
    frame: dict[str, Any] = {"cmd": "subscribe_push"}
    if kinds is not None:
        frame["kinds"] = kinds
    frame["subscription"] = subscription
    return frame
