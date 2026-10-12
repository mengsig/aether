# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class SubmissionResolutionUnknown(TypedDict, total=False):
    """
    No receipt, and nothing proves there never was one. Only a person can
    decide whether to send it again.
    """
    horizon: str
    outcome: Literal["unknown"]
    reason: _protocol.UnprovenAbsence
    retained_since: str
