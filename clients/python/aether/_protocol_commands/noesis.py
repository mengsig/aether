# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from ..protocol import (
    AnswerInput,
    Any,
    ArtifactId,
    ConceptStateKind,
    DeleteSessionGraph,
    DeleteSourceGraph,
    EvidenceBundle,
    JobState,
    NoesisSettingsPatch,
    SessionMode,
)

def noesis_answer(*, answer: AnswerInput, question: str, session: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Answer the session's open question."""
    frame: dict[str, Any] = {"cmd": "noesis_answer"}
    frame["answer"] = answer
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["question"] = question
    frame["session"] = session
    return frame

def noesis_cancel_job(*, job: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """The `noesis_cancel_job` command."""
    frame: dict[str, Any] = {"cmd": "noesis_cancel_job"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["job"] = job
    return frame

def noesis_configure(*, patch: NoesisSettingsPatch, idempotency_key: str | None = None) -> dict[str, Any]:
    """Write a subset of the `[noesis]` settings and answer with the new
    status. Works while Noesis is disabled. Host only when the patch sets
    `enabled`, a budget, billing or prompt overrides.
    """
    frame: dict[str, Any] = {"cmd": "noesis_configure"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["patch"] = patch
    return frame

def noesis_correct_learner(*, concept: str, reason: str, state: ConceptStateKind, idempotency_key: str | None = None) -> dict[str, Any]:
    """Record the operator's correction of a concept's state."""
    frame: dict[str, Any] = {"cmd": "noesis_correct_learner"}
    frame["concept"] = concept
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["reason"] = reason
    frame["state"] = state
    return frame

def noesis_create_session(*, goal: str, mode: SessionMode, context: EvidenceBundle | None = None, idempotency_key: str | None = None, practice_targets: list[str] | None = None) -> dict[str, Any]:
    """Start a teaching or practice session. Answered with `noesis_session`."""
    frame: dict[str, Any] = {"cmd": "noesis_create_session"}
    if context is not None:
        frame["context"] = context
    frame["goal"] = goal
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["mode"] = mode
    if practice_targets is not None:
        frame["practice_targets"] = practice_targets
    return frame

def noesis_delete_proposition(*, proposition: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Delete a proposition; its chains go with it."""
    frame: dict[str, Any] = {"cmd": "noesis_delete_proposition"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["proposition"] = proposition
    return frame

def noesis_delete_session(*, graph: DeleteSessionGraph, session: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """The `noesis_delete_session` command."""
    frame: dict[str, Any] = {"cmd": "noesis_delete_session"}
    frame["graph"] = graph
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["session"] = session
    return frame

def noesis_delete_source(*, graph: DeleteSourceGraph, source: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Refused as a conflict while a job on the source runs."""
    frame: dict[str, Any] = {"cmd": "noesis_delete_source"}
    frame["graph"] = graph
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["source"] = source
    return frame

def noesis_extract(*, idempotency_key: str | None = None, session: str | None = None, source: str | None = None) -> dict[str, Any]:
    """Extract the graph of exactly one of `session` and `source`."""
    frame: dict[str, Any] = {"cmd": "noesis_extract"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    if session is not None:
        frame["session"] = session
    if source is not None:
        frame["source"] = source
    return frame

def noesis_get_session(*, session: str) -> dict[str, Any]:
    """The `noesis_get_session` command."""
    frame: dict[str, Any] = {"cmd": "noesis_get_session"}
    frame["session"] = session
    return frame

def noesis_graph(*, limit: int | None = None, query: str | None = None, session: str | None = None, source: str | None = None) -> dict[str, Any]:
    """Propositions, chains and clusters, filtered by text, session or
    source. `limit` is 1 to 5000.
    """
    frame: dict[str, Any] = {"cmd": "noesis_graph"}
    if limit is not None:
        frame["limit"] = limit
    if query is not None:
        frame["query"] = query
    if session is not None:
        frame["session"] = session
    if source is not None:
        frame["source"] = source
    return frame

def noesis_import_source(*, artifact: ArtifactId, name: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Import an artifact as a source and extract it."""
    frame: dict[str, Any] = {"cmd": "noesis_import_source"}
    frame["artifact"] = artifact
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["name"] = name
    return frame

def noesis_jobs(*, state: JobState | None = None) -> dict[str, Any]:
    """The `noesis_jobs` command."""
    frame: dict[str, Any] = {"cmd": "noesis_jobs"}
    if state is not None:
        frame["state"] = state
    return frame

def noesis_learner(*, concepts: list[str] | None = None, limit: int | None = None, query: str | None = None) -> dict[str, Any]:
    """Learner state of the named concepts, or of those matching `query`.
    `limit` is 1 to 1000.
    """
    frame: dict[str, Any] = {"cmd": "noesis_learner"}
    if concepts is not None:
        frame["concepts"] = concepts
    if limit is not None:
        frame["limit"] = limit
    if query is not None:
        frame["query"] = query
    return frame

def noesis_learner_evidence(*, concept: str, before: int | None = None, limit: int | None = None) -> dict[str, Any]:
    """A concept's evidence, newest first, from before the evidence id
    `before`. `limit` is 1 to 500.
    """
    frame: dict[str, Any] = {"cmd": "noesis_learner_evidence"}
    if before is not None:
        frame["before"] = before
    frame["concept"] = concept
    if limit is not None:
        frame["limit"] = limit
    return frame

def noesis_list_sessions(*, after: str | None = None, limit: int | None = None) -> dict[str, Any]:
    """One page of sessions, from just after `after`. `limit` is 1 to 200."""
    frame: dict[str, Any] = {"cmd": "noesis_list_sessions"}
    if after is not None:
        frame["after"] = after
    if limit is not None:
        frame["limit"] = limit
    return frame

def noesis_retry_turn(*, session: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Retry a stalled teaching turn without repeating the learner's input."""
    frame: dict[str, Any] = {"cmd": "noesis_retry_turn"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["session"] = session
    return frame

def noesis_send(*, session: str, text: str, idempotency_key: str | None = None) -> dict[str, Any]:
    """Say something to the session's teacher. 1 to 20000 characters."""
    frame: dict[str, Any] = {"cmd": "noesis_send"}
    if idempotency_key is not None:
        frame["idempotency_key"] = idempotency_key
    frame["session"] = session
    frame["text"] = text
    return frame

def noesis_status() -> dict[str, Any]:
    """Noesis state: whether it is enabled, its settings as stored and as
    effective, running jobs and today's spend. Needs `features::NOESIS`.
    """
    frame: dict[str, Any] = {"cmd": "noesis_status"}
    return frame
