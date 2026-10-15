# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RepositoryAlias(TypedDict, total=False):
    """
    An old repository name memory used to be filed under, and the id it now
    resolves to. Registered by an operator; never guessed.
    """
    alias: str
    canonical: str
    created_at: str
