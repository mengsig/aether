# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentId,
    Any,
    NoteContext,
    NoteLevel,
)

def append_note(*, agent: AgentId, text: str, context: NoteContext | None = None, level: NoteLevel | None = None, links: list[str] | None = None) -> dict[str, Any]:
    """Append a note to an agent's transcript without starting a turn. People
    see it at once; the model sees it only with `context: digest`, in the
    next turn's runtime state. Needs `features::NOTES`.
    """
    frame: dict[str, Any] = {"cmd": "append_note"}
    frame["agent"] = agent
    if context is not None:
        frame["context"] = context
    if level is not None:
        frame["level"] = level
    if links is not None:
        frame["links"] = links
    frame["text"] = text
    return frame
