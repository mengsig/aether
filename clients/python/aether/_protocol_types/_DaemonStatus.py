# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class DaemonStatus(TypedDict, total=False):
    agents_blocked: int
    agents_live: int
    agents_paused: int  # optional
    agents_running: int
    agents_suspended: int  # optional
    agents_waiting: int
    approval_default: _protocol.ApprovalDefault  # optional
    bridge: str | None  # optional
    default_model: _protocol.ModelRef | None  # optional
    engine_turns_running: int  # optional
    engines: list[str]  # optional
    instance_id: str
    last_seq: int
    low_disk: _protocol.LowDisk | None  # optional
    max_concurrent_engine_turns: int | None  # optional
    max_concurrent_turns: int | None  # optional
    mcp: list[_protocol.McpServerHealth]  # optional
    network_default: _protocol.NetworkPolicy  # optional
    pending_approvals: int
    pid: int
    problems: list[str]  # optional
    providers: list[_protocol.ProviderHealth]  # optional
    resource_leases: list[_protocol.ResourceHolding]  # optional
    run_budget_usd: float | None  # optional
    runs: list[_protocol.RunSummary]  # optional
    started_at: str
    tasks_active: int
    usage_total: _protocol.Usage
    version: str
    yolo: bool  # optional
