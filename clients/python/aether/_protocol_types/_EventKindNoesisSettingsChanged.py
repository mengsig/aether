# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EventKindNoesisSettingsChanged(TypedDict, total=False):
    effective: _protocol.NoesisSettingsView
    settings: _protocol.NoesisSettingsView
    type: Literal["noesis.settings_changed"]
