# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class FeedbackRanking(TypedDict, total=False):
    """
    A memory's current tally and the factor it gives, as the host reports it.
    """
    factor: float
    helped: int
    hurt: int
