# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class NoesisModelOffer(TypedDict, total=False):
    """
    A model as Noesis sees it: how it is paid for and whether a step on it
    could run now (an engine detected and signed in, an API provider keyed).
    """
    billing: _protocol.NoesisBilling
    id: str
    ready: bool
