# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def alias_repository(*, alias: str, dry_run: bool | None = None, repository: str | None = None, root: str | None = None) -> dict[str, Any]:
    """Make `alias`, an old repository id, resolve to the repository at
    `root` or to the id `repository`, and move every memory filed under
    the alias there. The move is one-way. Needs `repository_aliases`.
    """
    frame: dict[str, Any] = {"cmd": "alias_repository"}
    frame["alias"] = alias
    if dry_run is not None:
        frame["dry_run"] = dry_run
    if repository is not None:
        frame["repository"] = repository
    if root is not None:
        frame["root"] = root
    return frame
