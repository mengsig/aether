# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisSettingsView(TypedDict, total=False):
    """
    The `[noesis]` config table (§6.1) as one JSON object of the same shape.
    """
    billing: _protocol.BillingSettings
    budget: _protocol.BudgetSettings
    concurrency: _protocol.ConcurrencySettings
    disable: _protocol.DisableSettings
    enabled: bool
    engineering_context: bool
    extraction: _protocol.ExtractionSettings
    models: _protocol.ModelSettings
    priority: _protocol.PrioritySettings
    prompts: _protocol.PromptSettings
