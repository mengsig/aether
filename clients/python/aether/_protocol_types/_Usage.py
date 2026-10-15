# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class Usage(TypedDict, total=False):
    """
    Token accounting for one invocation, or a running total.

    `cost_usd` sums invocations with a configured price or provider-reported
    cost; `cost_basis` distinguishes those sources and `cost_complete` says
    whether any invocations were left out. List prices are estimates, even
    when every invocation was priced. A budget enforces on this accounting
    bound - an unpriced invocation
    cannot make what was already spent smaller - so nothing silently stops
    firing when a model or engine reports no price.
    """
    cache_read_tokens: int
    cache_write_1h_tokens: int  # optional
    cache_write_tokens: int
    cost_basis: _protocol.CostBasis  # optional
    cost_complete: bool
    cost_gap: _protocol.CostGap | None  # optional
    cost_usd: float
    estimated_cost_usd: float | None  # optional
    input_tokens: int
    output_tokens: int
    reasoning_tokens: int | None  # optional
