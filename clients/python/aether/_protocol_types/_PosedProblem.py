# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from .. import protocol as _protocol


class PosedProblem(TypedDict, total=False):
    """
    A posed problem with its conclusion once it has one.
    """
    conclusion: _protocol.ProblemConclusion | None  # optional
    problem: _protocol.Problem
