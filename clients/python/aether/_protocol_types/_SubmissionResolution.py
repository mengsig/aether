# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._SubmissionResolutionAbsentSafe import SubmissionResolutionAbsentSafe
from ._SubmissionResolutionExecuted import SubmissionResolutionExecuted
from ._SubmissionResolutionUnknown import SubmissionResolutionUnknown


SubmissionResolution = (
    SubmissionResolutionExecuted |
    SubmissionResolutionAbsentSafe |
    SubmissionResolutionUnknown
)
