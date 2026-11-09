# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisStatus(TypedDict, total=False):
    """
    Reply to `noesis_status` and `noesis_configure`.
    """
    disable_policy: _protocol.DisablePolicy
    effective: _protocol.NoesisSettingsView
    enabled: bool
    jobs_running: int
    models: list[_protocol.NoesisModelOffer]  # optional
    prompt_overrides: list[str]
    settings: _protocol.NoesisSettingsView
    settings_pending: bool  # optional
    spend_today: _protocol.SpendToday
    teaching_model: _protocol.NoesisModelChoice | None  # optional
