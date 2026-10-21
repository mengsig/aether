# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._EventKindAgentArchived import EventKindAgentArchived
from ._EventKindAgentBlocked import EventKindAgentBlocked
from ._EventKindAgentCancelled import EventKindAgentCancelled
from ._EventKindAgentCompleted import EventKindAgentCompleted
from ._EventKindAgentConfigUpdated import EventKindAgentConfigUpdated
from ._EventKindAgentCreated import EventKindAgentCreated
from ._EventKindAgentFailed import EventKindAgentFailed
from ._EventKindAgentInterrupted import EventKindAgentInterrupted
from ._EventKindAgentMessage import EventKindAgentMessage
from ._EventKindAgentPaused import EventKindAgentPaused
from ._EventKindAgentReady import EventKindAgentReady
from ._EventKindAgentRestarted import EventKindAgentRestarted
from ._EventKindAgentResumed import EventKindAgentResumed
from ._EventKindAgentStarted import EventKindAgentStarted
from ._EventKindAgentTurnCompleted import EventKindAgentTurnCompleted
from ._EventKindAgentUsageUpdated import EventKindAgentUsageUpdated
from ._EventKindAgentWaiting import EventKindAgentWaiting
from ._EventKindApprovalRequested import EventKindApprovalRequested
from ._EventKindApprovalResolved import EventKindApprovalResolved
from ._EventKindApprovalRuleAdded import EventKindApprovalRuleAdded
from ._EventKindApprovalRuleRevoked import EventKindApprovalRuleRevoked
from ._EventKindArtifactCreated import EventKindArtifactCreated
from ._EventKindArtifactDeduplicated import EventKindArtifactDeduplicated
from ._EventKindArtifactEvicted import EventKindArtifactEvicted
from ._EventKindBudgetExceeded import EventKindBudgetExceeded
from ._EventKindBudgetWarning import EventKindBudgetWarning
from ._EventKindCheckpointCreated import EventKindCheckpointCreated
from ._EventKindCompactionCompleted import EventKindCompactionCompleted
from ._EventKindCompactionFailed import EventKindCompactionFailed
from ._EventKindCompactionNative import EventKindCompactionNative
from ._EventKindCompactionRequested import EventKindCompactionRequested
from ._EventKindContextHandoff import EventKindContextHandoff
from ._EventKindCustom import EventKindCustom
from ._EventKindDaemonStarted import EventKindDaemonStarted
from ._EventKindDaemonStopped import EventKindDaemonStopped
from ._EventKindDeadlinePassed import EventKindDeadlinePassed
from ._EventKindInboxConsumed import EventKindInboxConsumed
from ._EventKindInboxDelivered import EventKindInboxDelivered
from ._EventKindInvocationCompleted import EventKindInvocationCompleted
from ._EventKindInvocationFailed import EventKindInvocationFailed
from ._EventKindLeaseExpired import EventKindLeaseExpired
from ._EventKindLeaseGranted import EventKindLeaseGranted
from ._EventKindLeaseReleased import EventKindLeaseReleased
from ._EventKindLeaseRenewed import EventKindLeaseRenewed
from ._EventKindLeaseResourceAcquired import EventKindLeaseResourceAcquired
from ._EventKindLeaseResourceReleased import EventKindLeaseResourceReleased
from ._EventKindLeaseResourceWaiting import EventKindLeaseResourceWaiting
from ._EventKindMcpServerConnected import EventKindMcpServerConnected
from ._EventKindMcpServerFailed import EventKindMcpServerFailed
from ._EventKindMemoryAccepted import EventKindMemoryAccepted
from ._EventKindMemoryCandidate import EventKindMemoryCandidate
from ._EventKindMemoryForgotten import EventKindMemoryForgotten
from ._EventKindMemoryPinned import EventKindMemoryPinned
from ._EventKindMemoryRejected import EventKindMemoryRejected
from ._EventKindMemoryRestored import EventKindMemoryRestored
from ._EventKindMemorySuperseded import EventKindMemorySuperseded
from ._EventKindMessageSent import EventKindMessageSent
from ._EventKindProviderBackoff import EventKindProviderBackoff
from ._EventKindProviderFallback import EventKindProviderFallback
from ._EventKindProviderQuota import EventKindProviderQuota
from ._EventKindProviderRecovered import EventKindProviderRecovered
from ._EventKindQuestionAsked import EventKindQuestionAsked
from ._EventKindRecoveryClassified import EventKindRecoveryClassified
from ._EventKindRunImported import EventKindRunImported
from ._EventKindSnapshotCreated import EventKindSnapshotCreated
from ._EventKindTaskAssigned import EventKindTaskAssigned
from ._EventKindTaskBlocked import EventKindTaskBlocked
from ._EventKindTaskCancelled import EventKindTaskCancelled
from ._EventKindTaskCompleted import EventKindTaskCompleted
from ._EventKindTaskCreated import EventKindTaskCreated
from ._EventKindTaskFailed import EventKindTaskFailed
from ._EventKindTaskRequeued import EventKindTaskRequeued
from ._EventKindTaskStarted import EventKindTaskStarted
from ._EventKindTaskUnblocked import EventKindTaskUnblocked
from ._EventKindToolCancelled import EventKindToolCancelled
from ._EventKindToolDenied import EventKindToolDenied
from ._EventKindToolFailed import EventKindToolFailed
from ._EventKindToolReplayed import EventKindToolReplayed
from ._EventKindToolRequested import EventKindToolRequested
from ._EventKindToolRetried import EventKindToolRetried
from ._EventKindToolStarted import EventKindToolStarted
from ._EventKindToolSucceeded import EventKindToolSucceeded


EventKind = (
    EventKindAgentCreated |
    EventKindAgentReady |
    EventKindAgentStarted |
    EventKindAgentTurnCompleted |
    EventKindAgentInterrupted |
    EventKindAgentWaiting |
    EventKindAgentResumed |
    EventKindAgentPaused |
    EventKindAgentArchived |
    EventKindAgentBlocked |
    EventKindAgentCompleted |
    EventKindAgentFailed |
    EventKindAgentRestarted |
    EventKindAgentCancelled |
    EventKindAgentConfigUpdated |
    EventKindAgentUsageUpdated |
    EventKindAgentMessage |
    EventKindTaskCreated |
    EventKindTaskAssigned |
    EventKindTaskStarted |
    EventKindTaskBlocked |
    EventKindTaskUnblocked |
    EventKindTaskRequeued |
    EventKindTaskCompleted |
    EventKindTaskFailed |
    EventKindTaskCancelled |
    EventKindToolRequested |
    EventKindToolStarted |
    EventKindToolSucceeded |
    EventKindToolFailed |
    EventKindToolCancelled |
    EventKindToolDenied |
    EventKindToolReplayed |
    EventKindToolRetried |
    EventKindSnapshotCreated |
    EventKindInvocationCompleted |
    EventKindInvocationFailed |
    EventKindCompactionRequested |
    EventKindCheckpointCreated |
    EventKindCompactionCompleted |
    EventKindCompactionNative |
    EventKindCompactionFailed |
    EventKindContextHandoff |
    EventKindMemoryCandidate |
    EventKindMemoryAccepted |
    EventKindMemoryRejected |
    EventKindMemoryForgotten |
    EventKindMemoryRestored |
    EventKindMemoryPinned |
    EventKindMemorySuperseded |
    EventKindMessageSent |
    EventKindInboxDelivered |
    EventKindInboxConsumed |
    EventKindQuestionAsked |
    EventKindApprovalRequested |
    EventKindApprovalRuleAdded |
    EventKindApprovalRuleRevoked |
    EventKindApprovalResolved |
    EventKindArtifactCreated |
    EventKindArtifactDeduplicated |
    EventKindArtifactEvicted |
    EventKindMcpServerConnected |
    EventKindMcpServerFailed |
    EventKindRunImported |
    EventKindLeaseGranted |
    EventKindLeaseRenewed |
    EventKindLeaseReleased |
    EventKindLeaseExpired |
    EventKindLeaseResourceAcquired |
    EventKindLeaseResourceReleased |
    EventKindLeaseResourceWaiting |
    EventKindProviderBackoff |
    EventKindProviderRecovered |
    EventKindProviderFallback |
    EventKindProviderQuota |
    EventKindRecoveryClassified |
    EventKindBudgetWarning |
    EventKindDeadlinePassed |
    EventKindBudgetExceeded |
    EventKindDaemonStarted |
    EventKindDaemonStopped |
    EventKindCustom
)
