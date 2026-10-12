# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    ArchiveTarget,
)

def archive(*, target: ArchiveTarget, archived: bool | None = None, idempotency_key: str | None = None) -> dict[str, Any]:
    """Clear finished work out of the live view, or bring it back. The mark
    is durable and hides nothing from the store, reports or replay.
    Only a finished agent can be archived.
    """
    frame: dict[str, Any] = {"cmd": "archive"}
    if archived is not None:
        frame["archived"] = archived
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["target"] = target
    return frame
