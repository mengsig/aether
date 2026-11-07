# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RunSummary(TypedDict, total=False):
    """
    What one run has spent and where that rate leads.
    """
    agents: int
    agents_live: int
    forecast_usd_next_hour: float
    rate_approximate: bool  # optional
    rate_usd_per_hour: float
    run: _protocol.RunId
    started_at: str
    usage: _protocol.Usage
