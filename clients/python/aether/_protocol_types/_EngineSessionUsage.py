# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class EngineSessionUsage(TypedDict, total=False):
    """
    An engine session's own cumulative token counters, as it last reported
    them, tagged with the session they belong to.

    Codex counts a *thread*, not a turn: the counters it reports on a resumed
    thread include every earlier turn. Persisting the last reading is what
    lets the next turn bill `total - this` instead of the thread's history.
    The session id is part of the record so a cleared or replaced session
    cannot be billed against another session's counters.
    """
    models: dict[str, _protocol.Usage] | None  # optional
    session: str
    total: _protocol.Usage
