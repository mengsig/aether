# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class RepositoryAliasCounts(TypedDict, total=False):
    """
    What registering an alias moves, or moved: every memory under the alias,
    in any state, and the agents whose stored scope still names it.
    """
    agents: int
    duplicates: int
    repository: int
    repository_role: int
