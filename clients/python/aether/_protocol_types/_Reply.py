# Generated from clients/protocol.schema.json by scripts/generate_clients.py.
# Do not edit: change the Rust types and regenerate.
from __future__ import annotations

from typing import Any, Literal, TypedDict
from ._ReplyAgent import ReplyAgent
from ._ReplyAgents import ReplyAgents
from ._ReplyApprovalRule import ReplyApprovalRule
from ._ReplyApprovalRules import ReplyApprovalRules
from ._ReplyApprovals import ReplyApprovals
from ._ReplyArtifact import ReplyArtifact
from ._ReplyArtifactBytes import ReplyArtifactBytes
from ._ReplyArtifacts import ReplyArtifacts
from ._ReplyAttention import ReplyAttention
from ._ReplyAuth import ReplyAuth
from ._ReplyCheckpoint import ReplyCheckpoint
from ._ReplyDevice import ReplyDevice
from ._ReplyDevices import ReplyDevices
from ._ReplyEngineTool import ReplyEngineTool
from ._ReplyEngineTools import ReplyEngineTools
from ._ReplyEvents import ReplyEvents
from ._ReplyGitSummary import ReplyGitSummary
from ._ReplyGithubWatcher import ReplyGithubWatcher
from ._ReplyInbox import ReplyInbox
from ._ReplyInterrupted import ReplyInterrupted
from ._ReplyInvocation import ReplyInvocation
from ._ReplyInvocations import ReplyInvocations
from ._ReplyMcpServers import ReplyMcpServers
from ._ReplyMemories import ReplyMemories
from ._ReplyMemoriesInContext import ReplyMemoriesInContext
from ._ReplyMemory import ReplyMemory
from ._ReplyMemorySeeded import ReplyMemorySeeded
from ._ReplyMessageSent import ReplyMessageSent
from ._ReplyMessagesSent import ReplyMessagesSent
from ._ReplyModels import ReplyModels
from ._ReplyOk import ReplyOk
from ._ReplyPairing import ReplyPairing
from ._ReplyPathCompletions import ReplyPathCompletions
from ._ReplyPaused import ReplyPaused
from ._ReplyProviders import ReplyProviders
from ._ReplyPushConfig import ReplyPushConfig
from ._ReplyRemote import ReplyRemote
from ._ReplyReplay import ReplyReplay
from ._ReplyRepositoryAliased import ReplyRepositoryAliased
from ._ReplyRepositoryAliases import ReplyRepositoryAliases
from ._ReplyRetentionPreview import ReplyRetentionPreview
from ._ReplyRunExported import ReplyRunExported
from ._ReplyRunImported import ReplyRunImported
from ._ReplyRunReport import ReplyRunReport
from ._ReplyRuns import ReplyRuns
from ._ReplySchedule import ReplySchedule
from ._ReplyScheduleHistory import ReplyScheduleHistory
from ._ReplySchedules import ReplySchedules
from ._ReplySnapshot import ReplySnapshot
from ._ReplyStatus import ReplyStatus
from ._ReplySubmissionResolved import ReplySubmissionResolved
from ._ReplySubscribed import ReplySubscribed
from ._ReplyTask import ReplyTask
from ._ReplyTasks import ReplyTasks
from ._ReplyTranscript import ReplyTranscript
from ._ReplyTree import ReplyTree
from ._ReplyWelcome import ReplyWelcome
from ._ReplyWorkspaceValidation import ReplyWorkspaceValidation
from ._ReplyWorkspaces import ReplyWorkspaces


Reply = (
    ReplyRetentionPreview |
    ReplyGithubWatcher |
    ReplySchedule |
    ReplySchedules |
    ReplyScheduleHistory |
    ReplyWelcome |
    ReplyStatus |
    ReplyOk |
    ReplyInterrupted |
    ReplyPaused |
    ReplyAgent |
    ReplyAgents |
    ReplyTree |
    ReplyTask |
    ReplyTasks |
    ReplyMessageSent |
    ReplyMessagesSent |
    ReplyApprovals |
    ReplyApprovalRule |
    ReplyApprovalRules |
    ReplySubscribed |
    ReplyEvents |
    ReplyTranscript |
    ReplyInbox |
    ReplySnapshot |
    ReplyInvocation |
    ReplyInvocations |
    ReplyCheckpoint |
    ReplyArtifact |
    ReplyArtifacts |
    ReplyArtifactBytes |
    ReplyMemories |
    ReplyRepositoryAliased |
    ReplyRepositoryAliases |
    ReplyMemoriesInContext |
    ReplyMemory |
    ReplyMemorySeeded |
    ReplyRuns |
    ReplyRunReport |
    ReplyReplay |
    ReplyRunExported |
    ReplyRunImported |
    ReplyMcpServers |
    ReplyModels |
    ReplyProviders |
    ReplyAuth |
    ReplyEngineTool |
    ReplyEngineTools |
    ReplyAttention |
    ReplyPathCompletions |
    ReplyWorkspaces |
    ReplyWorkspaceValidation |
    ReplyGitSummary |
    ReplyRemote |
    ReplyPairing |
    ReplyDevice |
    ReplyDevices |
    ReplyPushConfig |
    ReplySubmissionResolved
)
