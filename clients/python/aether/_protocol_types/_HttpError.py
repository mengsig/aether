# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class HttpError(TypedDict, total=False):
    """
    Every non-2xx HTTP response body: `{"error": {"code", "message"}}`.
    """
    error: _protocol.HttpErrorBody
