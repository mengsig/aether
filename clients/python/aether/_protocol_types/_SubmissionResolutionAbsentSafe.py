# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SubmissionResolutionAbsentSafe(TypedDict, total=False):
    """
    It never ran: no receipt, and no receipt recorded since the request's
    horizon was ever dropped. Sending it again under the same key is safe.
    """
    horizon: str
    outcome: Literal["absent_safe"]
    retained_since: str
