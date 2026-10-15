# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ModelCost(TypedDict, total=False):
    """
    Price per million tokens in USD.
    """
    above_threshold: _protocol.ModelCostTier | None  # optional
    cache_read_per_mtok: float  # optional
    cache_write_1h_per_mtok: float | None  # optional
    cache_write_per_mtok: float  # optional
    input_per_mtok: float
    output_per_mtok: float
