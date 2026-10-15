// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AddApprovalRuleCommand,
  AliasRepositoryCommand,
  ArchiveCommand,
  AuthStatusCommand,
  CancelCommand,
  CancelPairingCommand,
  CompactCommand,
  CompletePathCommand,
  ConfigureGithubWatcherCommand,
  ConfigureRemoteCommand,
  ConfirmPairingCommand,
  CreateAgentCommand,
  CreateMemoryCommand,
  CreateScheduleCommand,
  CreateTaskCommand,
  EngineToolCommand,
  EngineToolsCommand,
  ExportRunCommand,
  ForgetMemoryCommand,
  GetAgentCommand,
  GetArtifactCommand,
  GetAttentionCommand,
  GetCheckpointCommand,
  GetEventsCommand,
  GetGitSummaryCommand,
  GetGithubWatcherCommand,
  GetInboxCommand,
  GetInvocationCommand,
  GetPairingCommand,
  GetPushConfigCommand,
  GetRemoteCommand,
  GetRunReportCommand,
  GetScheduleCommand,
  GetSnapshotCommand,
  GetTaskCommand,
  GetTranscriptCommand,
  GetTreeCommand,
  HelloCommand,
  ImportRunCommand,
  InterruptCommand,
  LatestCheckpointCommand,
  ListAgentsCommand,
  ListApprovalRulesCommand,
  ListApprovalsCommand,
  ListArtifactsCommand,
  ListDevicesCommand,
  ListInvocationsCommand,
  ListModelsCommand,
  ListRepositoryAliasesCommand,
  ListRunsCommand,
  ListSchedulesCommand,
  ListTasksCommand,
  ListWorkspacesCommand,
  McpServersCommand,
  MemoriesCommand,
  PauseCommand,
  PinMemoryCommand,
  PingCommand,
  PreviewRetentionCommand,
  ProvidersCommand,
  PublishArtifactCommand,
  ReadArtifactCommand,
  ReplayCommand,
  ReplyToQuestionCommand,
  ResolveApprovalCommand,
  ResolveSubmissionCommand,
  RestartCommand,
  RestoreMemoryCommand,
  ResumeCommand,
  RevokeApprovalRuleCommand,
  RevokeDeviceCommand,
  ScheduleHistoryCommand,
  SendMessageCommand,
  SetApprovalsCommand,
  ShutdownCommand,
  StartPairingCommand,
  StatusCommand,
  SubscribeCommand,
  SubscribePushCommand,
  SyncRepositoryKnowledgeCommand,
  TestPushCommand,
  TriggerCommand,
  UnsubscribeCommand,
  UnsubscribePushCommand,
  UpdateAgentCommand,
  UpdateDeviceCommand,
  UpdateScheduleCommand,
  ValidateWorkspaceCommand,
  WakeCommand,
} from "../../protocol.ts";

export type Command =
  | AddApprovalRuleCommand
  | AliasRepositoryCommand
  | ArchiveCommand
  | AuthStatusCommand
  | CancelCommand
  | CancelPairingCommand
  | CompactCommand
  | CompletePathCommand
  | ConfigureGithubWatcherCommand
  | ConfigureRemoteCommand
  | ConfirmPairingCommand
  | CreateAgentCommand
  | CreateMemoryCommand
  | CreateScheduleCommand
  | CreateTaskCommand
  | EngineToolCommand
  | EngineToolsCommand
  | ExportRunCommand
  | ForgetMemoryCommand
  | GetAgentCommand
  | GetArtifactCommand
  | GetAttentionCommand
  | GetCheckpointCommand
  | GetEventsCommand
  | GetGitSummaryCommand
  | GetGithubWatcherCommand
  | GetInboxCommand
  | GetInvocationCommand
  | GetPairingCommand
  | GetPushConfigCommand
  | GetRemoteCommand
  | GetRunReportCommand
  | GetScheduleCommand
  | GetSnapshotCommand
  | GetTaskCommand
  | GetTranscriptCommand
  | GetTreeCommand
  | HelloCommand
  | ImportRunCommand
  | InterruptCommand
  | LatestCheckpointCommand
  | ListAgentsCommand
  | ListApprovalRulesCommand
  | ListApprovalsCommand
  | ListArtifactsCommand
  | ListDevicesCommand
  | ListInvocationsCommand
  | ListModelsCommand
  | ListRepositoryAliasesCommand
  | ListRunsCommand
  | ListSchedulesCommand
  | ListTasksCommand
  | ListWorkspacesCommand
  | McpServersCommand
  | MemoriesCommand
  | PauseCommand
  | PinMemoryCommand
  | PingCommand
  | PreviewRetentionCommand
  | ProvidersCommand
  | PublishArtifactCommand
  | ReadArtifactCommand
  | ReplayCommand
  | ReplyToQuestionCommand
  | ResolveApprovalCommand
  | ResolveSubmissionCommand
  | RestartCommand
  | RestoreMemoryCommand
  | ResumeCommand
  | RevokeApprovalRuleCommand
  | RevokeDeviceCommand
  | ScheduleHistoryCommand
  | SendMessageCommand
  | SetApprovalsCommand
  | ShutdownCommand
  | StartPairingCommand
  | StatusCommand
  | SubscribeCommand
  | SubscribePushCommand
  | SyncRepositoryKnowledgeCommand
  | TestPushCommand
  | TriggerCommand
  | UnsubscribeCommand
  | UnsubscribePushCommand
  | UpdateAgentCommand
  | UpdateDeviceCommand
  | UpdateScheduleCommand
  | ValidateWorkspaceCommand
  | WakeCommand;
