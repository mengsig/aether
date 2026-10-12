# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NewApprovalRule(TypedDict, total=False):
    """
    A remembered decision written directly, without answering a call first.
    """
    decision: _protocol.ApprovalDecision
    idempotency_key: str | None  # optional
    match: _protocol.RuleMatch
    note: str | None  # optional
    scope: _protocol.RuleScope
