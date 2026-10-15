# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class TreeNode(TypedDict, total=False):
    """
    One node of the live graph.
    """
    agent: _protocol.Agent
    children: list[_protocol.AgentId]
    hidden_usage: _protocol.Usage | None  # optional
    lease: _protocol.Lease | None  # optional
    subtree_usage: _protocol.Usage
    task: _protocol.Task | None  # optional
    waiting_on: list[_protocol.TaskId]  # optional
