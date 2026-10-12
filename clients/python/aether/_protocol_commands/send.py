# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
    ImageAttachment,
)

def send_message(*, content: str, to: AgentId, attachments: list[ImageAttachment] | None = None, idempotency_key: str | None = None, subtree: bool | None = None) -> dict[str, Any]:
    """Deliver a user message to an agent's inbox; wakes it if waiting.
    With `subtree`, the message is fanned out to every live agent beneath
    it too, each with its own inbox item and events (§3.5).
    """
    frame: dict[str, Any] = {"cmd": "send_message"}
    if attachments is not None:
        frame["attachments"] = attachments
    frame["content"] = content
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if subtree is not None:
        frame["subtree"] = subtree
    frame["to"] = to
    return frame
