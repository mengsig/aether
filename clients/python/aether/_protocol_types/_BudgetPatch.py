# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class BudgetPatch(TypedDict, total=False):
    background_tokens_daily: int | None  # optional
    background_usd_daily: float | None  # optional
    interactive_usd_daily: float | None  # optional
