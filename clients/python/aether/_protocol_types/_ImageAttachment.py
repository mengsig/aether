# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ImageAttachment(TypedDict, total=False):
    """
    One image as a client sends it with a `send_message` command.
    """
    data: str
    media_type: str
    name: str | None  # optional
