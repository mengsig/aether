# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ViewMode(TypedDict, total=False):
    builtin: bool
    density: _protocol.ViewModeDensity
    description: str | None  # optional
    label: str | None  # optional
    name: str
    scope: _protocol.ViewModeScope | None  # optional
    show: list[_protocol.ViewModeCategory]
