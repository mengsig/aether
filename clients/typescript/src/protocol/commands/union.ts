// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AcceptMemoryCommand,
  AddApprovalRuleCommand,
  AliasRepositoryCommand,
  AnswerNotificationCommand,
  AppendNoteCommand,
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
  GetNotificationCommand,
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
  ListNotificationsCommand,
  ListRepositoryAliasesCommand,
  ListRunsCommand,
  ListSchedulesCommand,
  ListTasksCommand,
  ListWorkspacesCommand,
  MarkNotificationReadCommand,
  McpServersCommand,
  MemoriesCommand,
  MemoriesInContextCommand,
  MemoryFeedbackCommand,
  PauseCommand,
  PinMemoryCommand,
  PingCommand,
  PostNotificationCommand,
  PreviewRetentionCommand,
  ProvidersCommand,
  PublishArtifactCommand,
  ReadArtifactCommand,
  RecordMemoryFeedbackCommand,
  RefreshProviderUsageCommand,
  RejectMemoryCommand,
  ReplayCommand,
  ReplyToQuestionCommand,
  ResolveApprovalCommand,
  ResolveNotificationCommand,
  ResolveSubmissionCommand,
  RestartCommand,
  RestoreMemoryCommand,
  ResumeCommand,
  RetractMemoryFeedbackCommand,
  RevokeApprovalRuleCommand,
  RevokeDeviceCommand,
  ScheduleHistoryCommand,
  SendMessageCommand,
  SetApprovalsCommand,
  ShutdownCommand,
  StartPairingCommand,
  StatusCommand,
  StorageGcCommand,
  StorageStatusCommand,
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
  WorkspaceDiffCommand,
  WorkspaceDiscardCommand,
  WorkspaceIntegrateCommand,
} from "../../protocol.ts";

export type Command =
  | AcceptMemoryCommand
  | AddApprovalRuleCommand
  | AliasRepositoryCommand
  | AnswerNotificationCommand
  | AppendNoteCommand
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
  | GetNotificationCommand
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
  | ListNotificationsCommand
  | ListRepositoryAliasesCommand
  | ListRunsCommand
  | ListSchedulesCommand
  | ListTasksCommand
  | ListWorkspacesCommand
  | MarkNotificationReadCommand
  | McpServersCommand
  | MemoriesCommand
  | MemoriesInContextCommand
  | MemoryFeedbackCommand
  | PauseCommand
  | PinMemoryCommand
  | PingCommand
  | PostNotificationCommand
  | PreviewRetentionCommand
  | ProvidersCommand
  | PublishArtifactCommand
  | ReadArtifactCommand
  | RecordMemoryFeedbackCommand
  | RefreshProviderUsageCommand
  | RejectMemoryCommand
  | ReplayCommand
  | ReplyToQuestionCommand
  | ResolveApprovalCommand
  | ResolveNotificationCommand
  | ResolveSubmissionCommand
  | RestartCommand
  | RestoreMemoryCommand
  | ResumeCommand
  | RetractMemoryFeedbackCommand
  | RevokeApprovalRuleCommand
  | RevokeDeviceCommand
  | ScheduleHistoryCommand
  | SendMessageCommand
  | SetApprovalsCommand
  | ShutdownCommand
  | StartPairingCommand
  | StatusCommand
  | StorageGcCommand
  | StorageStatusCommand
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
  | WakeCommand
  | WorkspaceDiffCommand
  | WorkspaceDiscardCommand
  | WorkspaceIntegrateCommand;
