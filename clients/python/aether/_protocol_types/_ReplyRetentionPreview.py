# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyRetentionPreview(TypedDict, total=False):
    data: _protocol.RetentionPreview
    reply: Literal["retention_preview"]
