# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What paid for a usage record when it was written: an engine CLI on the
# user's own subscription, or a provider billed per request.
Billing = Literal["subscription", "api"]
