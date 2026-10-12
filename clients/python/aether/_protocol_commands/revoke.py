# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    ApprovalRuleId,
    DeviceId,
)

def revoke_approval_rule(*, id: ApprovalRuleId, reason: str | None = None) -> dict[str, Any]:
    """The `revoke_approval_rule` command."""
    frame: dict[str, Any] = {"cmd": "revoke_approval_rule"}
    frame["id"] = id
    if reason is not None:
        frame["reason"] = reason
    return frame

def revoke_device(*, id: DeviceId, reason: str | None = None) -> dict[str, Any]:
    """Revoke a device and close its connections and subscriptions at once.
    Host only.
    """
    frame: dict[str, Any] = {"cmd": "revoke_device"}
    frame["id"] = id
    if reason is not None:
        frame["reason"] = reason
    return frame
