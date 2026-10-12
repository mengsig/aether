# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._ContentBlockImage import ContentBlockImage
from ._ContentBlockImageRef import ContentBlockImageRef
from ._ContentBlockText import ContentBlockText
from ._ContentBlockThinking import ContentBlockThinking
from ._ContentBlockToolResult import ContentBlockToolResult
from ._ContentBlockToolUse import ContentBlockToolUse


ContentBlock = (
    ContentBlockText |
    ContentBlockThinking |
    ContentBlockToolUse |
    ContentBlockToolResult |
    ContentBlockImage |
    ContentBlockImageRef
)
