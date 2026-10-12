# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    ApprovalDecision,
    RuleMatch,
    RuleScope,
)

def add_approval_rule(*, decision: ApprovalDecision, match: RuleMatch, scope: RuleScope, idempotency_key: str | None = None, note: str | None = None) -> dict[str, Any]:
    """A remembered decision written directly, without answering a call first."""
    frame: dict[str, Any] = {"cmd": "add_approval_rule"}
    frame["decision"] = decision
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["match"] = match
    if note is not None:
        frame["note"] = note
    frame["scope"] = scope
    return frame
