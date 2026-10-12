# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
)

def validate_workspace(*, path: str) -> dict[str, Any]:
    """Whether an agent can be started in `path`, and its canonical form."""
    frame: dict[str, Any] = {"cmd": "validate_workspace"}
    frame["path"] = path
    return frame
