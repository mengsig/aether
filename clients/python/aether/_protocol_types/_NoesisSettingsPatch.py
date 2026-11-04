# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisSettingsPatch(TypedDict, total=False):
    """
    A subset of the `[noesis]` table to write. Absent keys keep their value.
    """
    billing: _protocol.BillingPatch | None  # optional
    budget: _protocol.BudgetPatch | None  # optional
    concurrency: _protocol.ConcurrencyPatch | None  # optional
    disable: _protocol.DisablePatch | None  # optional
    enabled: bool | None  # optional
    engineering_context: bool | None  # optional
    extraction: _protocol.ExtractionPatch | None  # optional
    models: _protocol.ModelsPatch | None  # optional
    priority: _protocol.PriorityPatch | None  # optional
    prompts: _protocol.PromptsPatch | None  # optional
