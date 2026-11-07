# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class ReplyTranscriptPage(TypedDict, total=False):
    """
    Reply to `get_transcript_page`: entries in ascending `seq`, whether
    older or newer ones exist outside it, and the seq of the agent's newest
    entry (0 for none).
    """
    data: dict[str, Any]
    reply: Literal["transcript_page"]
