# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    Any,
    UsageGroup,
    UsageScope,
)

def usage_breakdown(*, by: UsageGroup, limit: int | None = None, scope: UsageScope | None = None, since: str | None = None, until: str | None = None) -> dict[str, Any]:
    """Spend summed from `invocation.completed` events, grouped by `by`, in
    `since..until` (completion time, `until` exclusive). `limit` is 1 to
    1000 rows, 1000 when absent. Needs `features::USAGE_BREAKDOWN`.
    """
    frame: dict[str, Any] = {"cmd": "usage_breakdown"}
    frame["by"] = by
    if limit is not None:
        frame["limit"] = limit
    if scope is not None:
        frame["scope"] = scope
    if since is not None:
        frame["since"] = since
    if until is not None:
        frame["until"] = until
    return frame
