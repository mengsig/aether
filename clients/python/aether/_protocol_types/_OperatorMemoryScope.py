# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._OperatorMemoryScopeAgent import OperatorMemoryScopeAgent
from ._OperatorMemoryScopeGlobal import OperatorMemoryScopeGlobal
from ._OperatorMemoryScopeRepository import OperatorMemoryScopeRepository
from ._OperatorMemoryScopeRepositoryRole import OperatorMemoryScopeRepositoryRole
from ._OperatorMemoryScopeRole import OperatorMemoryScopeRole


OperatorMemoryScope = (
    OperatorMemoryScopeAgent |
    OperatorMemoryScopeGlobal |
    OperatorMemoryScopeRepository |
    OperatorMemoryScopeRole |
    OperatorMemoryScopeRepositoryRole
)
