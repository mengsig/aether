# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# How a note reads on screen: a plain remark, or the result of something a
# host did for the operator. It never reaches the model on its own.
NoteLevel = Literal["info", "applied", "refused"]
