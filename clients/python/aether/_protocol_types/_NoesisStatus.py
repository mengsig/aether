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
    prompt_overrides: list[str]
    settings: _protocol.NoesisSettingsView
    spend_today: _protocol.SpendToday
