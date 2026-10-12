# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AgentConfig,
    AgentId,
    Any,
    ApprovalPolicy,
    CreateScheduleSpec,
    ModelRef,
    OperatorMemorySpec,
    Priority,
    RunId,
    Scope,
    TaskId,
)

def create_agent(*, role: str, approval: ApprovalPolicy | None = None, config: AgentConfig | None = None, fallbacks: list[ModelRef] | None = None, idempotency_key: str | None = None, model: ModelRef | None = None, name: str | None = None, objective: str | None = None, parent: AgentId | None = None, run: RunId | None = None, scope: Scope | None = None, start: bool | None = None) -> dict[str, Any]:
    """Request to create an agent, optionally with an initial task.
    A child requires explicit `config.effort`: low, medium, high, xhigh or max.
    """
    frame: dict[str, Any] = {"cmd": "create_agent"}
    if approval is not None:
        frame["approval"] = approval
    if config is not None:
        frame["config"] = config
    if fallbacks is not None:
        frame["fallbacks"] = fallbacks
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if model is not None:
        frame["model"] = model
    if name is not None:
        frame["name"] = name
    if objective is not None:
        frame["objective"] = objective
    if parent is not None:
        frame["parent"] = parent
    frame["role"] = role
    if run is not None:
        frame["run"] = run
    if scope is not None:
        frame["scope"] = scope
    if start is not None:
        frame["start"] = start
    return frame

def create_memory(*, spec: OperatorMemorySpec, idempotency_key: str | None = None) -> dict[str, Any]:
    """Admit an operator-authored fact. Rejections return an error without mutation."""
    frame: dict[str, Any] = {"cmd": "create_memory"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["spec"] = spec
    return frame

def create_schedule(*, spec: CreateScheduleSpec, idempotency_key: str | None = None) -> dict[str, Any]:
    """The `create_schedule` command."""
    frame: dict[str, Any] = {"cmd": "create_schedule"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["spec"] = spec
    return frame

def create_task(*, objective: str, dependencies: list[TaskId] | None = None, idempotency_key: str | None = None, owner: AgentId | None = None, parent_task: TaskId | None = None, priority: Priority | None = None) -> dict[str, Any]:
    """The `create_task` command."""
    frame: dict[str, Any] = {"cmd": "create_task"}
    if dependencies is not None:
        frame["dependencies"] = dependencies
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["objective"] = objective
    if owner is not None:
        frame["owner"] = owner
    if parent_task is not None:
        frame["parent_task"] = parent_task
    if priority is not None:
        frame["priority"] = priority
    return frame
