# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    ApprovalDecision,
    ApprovalId,
    NotificationId,
    RememberSpec,
)

def resolve_approval(*, decision: ApprovalDecision, id: ApprovalId, idempotency_key: str | None = None, note: str | None = None, remember: RememberSpec | None = None) -> dict[str, Any]:
    """The `resolve_approval` command."""
    frame: dict[str, Any] = {"cmd": "resolve_approval"}
    frame["decision"] = decision
    frame["id"] = id
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if note is not None:
        frame["note"] = note
    if remember is not None:
        frame["remember"] = remember
    return frame

def resolve_notification(*, id: NotificationId | None = None, key: str | None = None, source: str | None = None) -> dict[str, Any]:
    """Resolve a notification by `id`, or by the `source` and `key` it was
    posted under; exactly one of the two. Needs `features::NOTIFICATIONS`.
    """
    frame: dict[str, Any] = {"cmd": "resolve_notification"}
    if id is not None:
        frame["id"] = id
    if key is not None:
        frame["key"] = key
    if source is not None:
        frame["source"] = source
    return frame

def resolve_submission(*, command: dict[str, Any], horizon: str | None = None) -> dict[str, Any]:
    """Ask what became of a keyed command whose acknowledgement was lost.
    Read-only, within the caller's own keys; needs `command`'s standing.
    """
    frame: dict[str, Any] = {"cmd": "resolve_submission"}
    frame["command"] = command
    if horizon is not None:
        frame["horizon"] = horizon
    return frame
