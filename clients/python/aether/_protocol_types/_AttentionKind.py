# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What can wait on the operator, and what a push notification is about.
AttentionKind = Literal["approval", "question", "agent_failed", "agent_blocked", "task_completed", "task_failed", "test"]
