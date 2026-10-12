# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyPathCompletions(TypedDict, total=False):
    data: _protocol.PathCompletions
    reply: Literal["path_completions"]
