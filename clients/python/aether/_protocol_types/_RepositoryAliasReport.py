# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RepositoryAliasReport(TypedDict, total=False):
    """
    The answer to registering an alias, or to a dry run of it.
    """
    alias: str
    canonical: str
    counts: _protocol.RepositoryAliasCounts
    dry_run: bool
    existing: bool
