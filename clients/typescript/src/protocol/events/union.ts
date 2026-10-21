// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EventKindAgentArchived,
  EventKindAgentBlocked,
  EventKindAgentCancelled,
  EventKindAgentCompleted,
  EventKindAgentConfigUpdated,
  EventKindAgentCreated,
  EventKindAgentFailed,
  EventKindAgentInterrupted,
  EventKindAgentMessage,
  EventKindAgentPaused,
  EventKindAgentReady,
  EventKindAgentRestarted,
  EventKindAgentResumed,
  EventKindAgentStarted,
  EventKindAgentTurnCompleted,
  EventKindAgentUsageUpdated,
  EventKindAgentWaiting,
  EventKindApprovalRequested,
  EventKindApprovalResolved,
  EventKindApprovalRuleAdded,
  EventKindApprovalRuleRevoked,
  EventKindArtifactCreated,
  EventKindArtifactDeduplicated,
  EventKindArtifactEvicted,
  EventKindBudgetExceeded,
  EventKindBudgetWarning,
  EventKindCheckpointCreated,
  EventKindCompactionCompleted,
  EventKindCompactionFailed,
  EventKindCompactionNative,
  EventKindCompactionRequested,
  EventKindContextHandoff,
  EventKindCustom,
  EventKindDaemonStarted,
  EventKindDaemonStopped,
  EventKindDeadlinePassed,
  EventKindInboxConsumed,
  EventKindInboxDelivered,
  EventKindInvocationCompleted,
  EventKindInvocationFailed,
  EventKindLeaseExpired,
  EventKindLeaseGranted,
  EventKindLeaseReleased,
  EventKindLeaseRenewed,
  EventKindLeaseResourceAcquired,
  EventKindLeaseResourceReleased,
  EventKindLeaseResourceWaiting,
  EventKindMcpServerConnected,
  EventKindMcpServerFailed,
  EventKindMemoryAccepted,
  EventKindMemoryCandidate,
  EventKindMemoryForgotten,
  EventKindMemoryPinned,
  EventKindMemoryRejected,
  EventKindMemoryRestored,
  EventKindMemorySuperseded,
  EventKindMessageSent,
  EventKindProviderBackoff,
  EventKindProviderFallback,
  EventKindProviderQuota,
  EventKindProviderRecovered,
  EventKindQuestionAsked,
  EventKindRecoveryClassified,
  EventKindRunImported,
  EventKindSnapshotCreated,
  EventKindTaskAssigned,
  EventKindTaskBlocked,
  EventKindTaskCancelled,
  EventKindTaskCompleted,
  EventKindTaskCreated,
  EventKindTaskFailed,
  EventKindTaskRequeued,
  EventKindTaskStarted,
  EventKindTaskUnblocked,
  EventKindToolCancelled,
  EventKindToolDenied,
  EventKindToolFailed,
  EventKindToolReplayed,
  EventKindToolRequested,
  EventKindToolRetried,
  EventKindToolStarted,
  EventKindToolSucceeded,
} from "../../protocol.ts";


export type EventKind =
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
;
