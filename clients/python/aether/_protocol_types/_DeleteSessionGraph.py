# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What deleting a session does to the propositions it produced.
DeleteSessionGraph = Literal["preserve", "delete_nodes"]
