# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RemoteSettings(TypedDict, total=False):
    """
    The runtime-changeable remote settings, saved whole under a revision.
    """
    enabled: bool  # optional
    pairing: bool  # optional
    public_origins: list[str]  # optional
    push: bool  # optional
