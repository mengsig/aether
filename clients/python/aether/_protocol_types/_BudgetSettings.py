# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class BudgetSettings(TypedDict, total=False):
    background_tokens_daily: int
    background_usd_daily: float
    interactive_usd_daily: float
