# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._OperatorOriginDevice import OperatorOriginDevice
from ._OperatorOriginLocalSocket import OperatorOriginLocalSocket
from ._OperatorOriginTcp import OperatorOriginTcp


OperatorOrigin = OperatorOriginLocalSocket | OperatorOriginTcp | OperatorOriginDevice
