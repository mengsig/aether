# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ContentBlockImageRef(TypedDict, total=False):
    """
    An image a person sent with a message, kept by the daemon. Durable
    state carries this reference; the bytes are read back only to build
    a model request.
    """
    id: _protocol.AttachmentId
    media_type: str
    name: str | None  # optional
    size_bytes: int
    type: Literal["image_ref"]
