# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ErrorReply(TypedDict, total=False):
    code: _protocol.ErrorCode
    first_seq: int | None  # optional
    last_seq: int | None  # optional
    message: str
