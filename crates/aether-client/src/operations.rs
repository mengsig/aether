use crate::{Client, Result, expect_ok, unexpected};
use aether_core::agent::Agent;
use aether_core::approval::ApprovalRule;
use aether_core::id::{AgentId, ApprovalId, ApprovalRuleId, TaskId};
use aether_core::task::Task;
use aether_core::tool::{ApprovalDecision, ApprovalRequest};
use aether_protocol::{
    AuthInfo, Command, CreateTask, DaemonStatus, ModelInfo, NewApprovalRule, ProviderInfo,
    RememberSpec, Reply, TaskQuery,
};

impl Client {
    // ----- typed helpers ---------------------------------------------------

    pub async fn status(&self) -> Result<DaemonStatus> {
        match self.request(Command::Status).await? {
            Reply::Status(s) => Ok(s),
            other => Err(unexpected("status", &other)),
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        expect_ok(self.request(Command::Shutdown).await?)
    }

    pub async fn github_watcher(&self) -> Result<aether_core::github::GithubWatcherStatus> {
        match self.request(Command::GetGithubWatcher).await? {
            Reply::GithubWatcher(status) => Ok(status),
            other => Err(unexpected("github_watcher", &other)),
        }
    }

    pub async fn configure_github_watcher(
        &self,
        config: aether_core::github::GithubWatcherConfig,
        expected_revision: u64,
    ) -> Result<aether_core::github::GithubWatcherStatus> {
        match self
            .request(Command::ConfigureGithubWatcher {
                config,
                expected_revision,
            })
            .await?
        {
            Reply::GithubWatcher(status) => Ok(status),
            other => Err(unexpected("github_watcher", &other)),
        }
    }

    pub async fn create_schedule(
        &self,
        spec: aether_core::schedule::CreateScheduleSpec,
    ) -> Result<aether_core::schedule::Schedule> {
        match self
            .request(Command::CreateSchedule {
                spec,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Schedule(job) => Ok(job),
            other => Err(unexpected("schedule", &other)),
        }
    }

    pub async fn schedule(
        &self,
        id: aether_core::id::ScheduleId,
    ) -> Result<aether_core::schedule::Schedule> {
        match self.request(Command::GetSchedule { id }).await? {
            Reply::Schedule(job) => Ok(job),
            other => Err(unexpected("schedule", &other)),
        }
    }

    pub async fn schedules(
        &self,
        target: Option<AgentId>,
    ) -> Result<Vec<aether_core::schedule::Schedule>> {
        match self.request(Command::ListSchedules { target }).await? {
            Reply::Schedules(jobs) => Ok(jobs),
            other => Err(unexpected("schedules", &other)),
        }
    }

    pub async fn update_schedule(
        &self,
        id: aether_core::id::ScheduleId,
        action: aether_core::schedule::ScheduleAction,
    ) -> Result<aether_core::schedule::Schedule> {
        match self
            .request(Command::UpdateSchedule {
                id,
                action,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Schedule(job) => Ok(job),
            other => Err(unexpected("schedule", &other)),
        }
    }

    pub async fn schedule_history(
        &self,
        id: aether_core::id::ScheduleId,
        limit: usize,
    ) -> Result<Vec<aether_core::schedule::ScheduleOccurrence>> {
        match self.request(Command::ScheduleHistory { id, limit }).await? {
            Reply::ScheduleHistory(history) => Ok(history),
            other => Err(unexpected("schedule_history", &other)),
        }
    }

    /// Deliver an external trigger to an agent's inbox.
    pub async fn trigger(
        &self,
        agent: AgentId,
        source: String,
        payload: serde_json::Value,
    ) -> Result<()> {
        expect_ok(
            self.request(Command::Trigger {
                agent,
                source,
                payload,
                idempotency_key: None,
            })
            .await?,
        )
    }

    /// Turn approvals off (yolo) or back on, optionally over the subtree.
    pub async fn set_approvals(
        &self,
        agent: AgentId,
        policy: aether_core::agent::ApprovalPolicy,
        subtree: bool,
    ) -> Result<Vec<Agent>> {
        match self
            .request(Command::SetApprovals {
                agent,
                policy,
                subtree,
                idempotency_key: None,
            })
            .await?
        {
            Reply::Agents(agents) => Ok(agents),
            other => Err(unexpected("agents", &other)),
        }
    }

    pub async fn create_task(&self, req: CreateTask) -> Result<Task> {
        match self.request(Command::CreateTask(req)).await? {
            Reply::Task(t) => Ok(t),
            other => Err(unexpected("task", &other)),
        }
    }

    pub async fn get_task(&self, id: TaskId) -> Result<Task> {
        match self.request(Command::GetTask { id }).await? {
            Reply::Task(t) => Ok(t),
            other => Err(unexpected("task", &other)),
        }
    }

    pub async fn list_tasks(&self, query: TaskQuery) -> Result<Vec<Task>> {
        match self.request(Command::ListTasks { query }).await? {
            Reply::Tasks(t) => Ok(t),
            other => Err(unexpected("tasks", &other)),
        }
    }

    pub async fn list_approvals(&self) -> Result<Vec<ApprovalRequest>> {
        match self.request(Command::ListApprovals).await? {
            Reply::Approvals(a) => Ok(a),
            other => Err(unexpected("approvals", &other)),
        }
    }

    pub async fn resolve_approval(
        &self,
        id: ApprovalId,
        decision: ApprovalDecision,
        note: Option<String>,
    ) -> Result<()> {
        self.resolve_approval_remembering(id, decision, note, None)
            .await
    }

    /// Answer an approval and, with `remember`, keep the answer as a rule.
    pub async fn resolve_approval_remembering(
        &self,
        id: ApprovalId,
        decision: ApprovalDecision,
        note: Option<String>,
        remember: Option<RememberSpec>,
    ) -> Result<()> {
        expect_ok(
            self.request(Command::ResolveApproval {
                id,
                decision,
                note,
                remember,
                idempotency_key: None,
            })
            .await?,
        )
    }

    /// Configured MCP servers and their health. `refresh` reconnects first.
    pub async fn mcp_servers(
        &self,
        refresh: bool,
        server: Option<String>,
    ) -> Result<Vec<aether_core::mcp::McpServerHealth>> {
        match self
            .request(Command::McpServers { refresh, server })
            .await?
        {
            Reply::McpServers(s) => Ok(s),
            other => Err(unexpected("mcp servers", &other)),
        }
    }

    pub async fn list_approval_rules(&self, include_revoked: bool) -> Result<Vec<ApprovalRule>> {
        match self
            .request(Command::ListApprovalRules { include_revoked })
            .await?
        {
            Reply::ApprovalRules(r) => Ok(r),
            other => Err(unexpected("approval rules", &other)),
        }
    }

    pub async fn add_approval_rule(&self, rule: NewApprovalRule) -> Result<ApprovalRule> {
        match self.request(Command::AddApprovalRule(rule)).await? {
            Reply::ApprovalRule(r) => Ok(r),
            other => Err(unexpected("approval rule", &other)),
        }
    }

    pub async fn revoke_approval_rule(
        &self,
        id: ApprovalRuleId,
        reason: Option<String>,
    ) -> Result<ApprovalRule> {
        match self
            .request(Command::RevokeApprovalRule { id, reason })
            .await?
        {
            Reply::ApprovalRule(r) => Ok(r),
            other => Err(unexpected("approval rule", &other)),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        match self.request(Command::ListModels).await? {
            Reply::Models(m) => Ok(m),
            other => Err(unexpected("models", &other)),
        }
    }

    pub async fn auth_status(&self) -> Result<Vec<AuthInfo>> {
        match self.request(Command::AuthStatus).await? {
            Reply::Auth(a) => Ok(a),
            other => Err(unexpected("auth", &other)),
        }
    }

    /// Every provider the registry knows, with its live health.
    pub async fn providers(&self) -> Result<Vec<ProviderInfo>> {
        match self.request(Command::Providers).await? {
            Reply::Providers(p) => Ok(p),
            other => Err(unexpected("providers", &other)),
        }
    }
}
