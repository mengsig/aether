# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class PairingCandidate(TypedDict, total=False):
    """
    The device asking to be enrolled, as the host must confirm it.
    """
    claimed_at: str
    fingerprint: str
    name: str
    phrase: str
