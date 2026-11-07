# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict


# What an answer revealed: one value for a recall question, several for a
# multiple choice one.
AnswerKey = list[str] | str
