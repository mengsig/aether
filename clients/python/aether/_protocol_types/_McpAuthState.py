# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# Whether a server that names a token variable got its token.
McpAuthState = Literal["token", "auth_failed", "anonymous"]
