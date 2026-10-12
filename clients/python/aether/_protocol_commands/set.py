# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
    ApprovalPolicy,
)

def set_approvals(*, agent: AgentId, policy: ApprovalPolicy, idempotency_key: str | None = None, subtree: bool | None = None) -> dict[str, Any]:
    """Turn approvals off (yolo) or back on for an agent and, with
    `subtree`, every live agent beneath it. A separate command
    from `UpdateAgent` because it carries the subtree reach and the
    never-widen-past-the-parent rule.
    """
    frame: dict[str, Any] = {"cmd": "set_approvals"}
    frame["agent"] = agent
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["policy"] = policy
    if subtree is not None:
        frame["subtree"] = subtree
    return frame
