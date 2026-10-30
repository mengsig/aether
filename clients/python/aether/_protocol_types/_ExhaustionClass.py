# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# The subset of [`ModelErrorClass`] a probe can recover from. `ContextLimit`
# is deliberately unrepresentable: it is not credits, and waiting cannot fix it.
# `Credentials` is a sign-in the provider refused, which an operator renews.
ExhaustionClass = Literal["quota_exhausted", "usage_limit", "credentials"]
