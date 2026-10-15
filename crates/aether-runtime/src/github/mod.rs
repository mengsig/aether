//! GitHub-specific polling at the runtime boundary. External text reaches only
//! ExternalTrigger inbox items; actor permissions are fetched from GitHub, never
//! inferred from that text. Delivery and its durable dedup marker commit together.
mod checkout;
#[cfg(test)]
mod operator_tests;
mod poll;
mod source;
mod standing;
#[cfg(test)]
mod standing_tests;
#[cfg(test)]
mod tests;

use crate::{
    CreateAgentSpec, Result, Runtime, RuntimeError,
    commands::{Preflight, PreparedAgent},
};
use aether_core::{
    agent::{Agent, AgentStatus, Scope},
    event::{Actor, EventDraft, EventKind},
    github::{
        GithubDispatchMode, GithubRepositoryConfig, GithubRepositoryStatus, GithubWatcherConfig,
        GithubWatcherStatus,
    },
    id::AgentId,
    inbox::InboxKind,
    schedule::SpawnSpec,
    time::{Timestamp, now},
};
use aether_store::{Commit, Mutation};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const CONFIG_KEY: &str = "github.watcher.config";
#[derive(Default, Serialize, Deserialize)]
struct Settings {
    revision: u64,
    config: GithubWatcherConfig,
}
#[derive(Serialize, Deserialize)]
struct RepositoryState {
    baseline: Timestamp,
    cursor: Timestamp,
    review_cursor: Timestamp,
    failures: u32,
    status: GithubRepositoryStatus,
}
fn state_key(repo: &str) -> String {
    format!("github.watcher.repo:{}", repo.to_ascii_lowercase())
}
/// Durable (repository, thread) -> spawned agent mapping for `Thread`
/// dispatch. Distinct from `github.watcher.thread:{repo}:{number}`, the
/// per-thread dispatch-rate cap key, so the two never collide.
fn thread_agent_key(repo: &str, number: u64) -> String {
    format!("github.watcher.thread-agent:{repo}:{number}")
}
fn automatic_agent_key(repo: &str, number: u64) -> String {
    format!("github.watcher.automatic-agent:{repo}:{number}")
}
fn automatic_generation_key(repo: &str, number: u64) -> String {
    format!("github.watcher.automatic-generation:{repo}:{number}")
}
enum Target {
    Existing(AgentId),
    Prepared {
        agent: Box<PreparedAgent>,
        generation: Option<u64>,
    },
}
fn encode(key: String, value: &impl Serialize) -> Result<Mutation> {
    Ok(Mutation::SetMeta {
        key,
        value: serde_json::to_string(value)
            .map_err(|e| RuntimeError::Internal(format!("serialize GitHub watcher: {e}")))?,
    })
}

impl Runtime {
    fn github_settings(&self) -> Result<Settings> {
        self.store
            .meta(CONFIG_KEY)?
            .map(|s| {
                serde_json::from_str(&s).map_err(|e| {
                    RuntimeError::Internal(format!("corrupt GitHub watcher settings: {e}"))
                })
            })
            .unwrap_or_else(|| Ok(Settings::default()))
    }
    fn github_repository_state(&self, repo: &str) -> Result<RepositoryState> {
        let raw = self
            .store
            .meta(&state_key(repo))?
            .ok_or_else(|| RuntimeError::Internal(format!("missing GitHub state for {repo}")))?;
        serde_json::from_str(&raw)
            .map_err(|e| RuntimeError::Internal(format!("corrupt GitHub state for {repo}: {e}")))
    }
    pub fn github_watcher_status(&self) -> Result<GithubWatcherStatus> {
        let _hold = self.hold();
        let settings = self.github_settings()?;
        let repositories = settings
            .config
            .repositories
            .iter()
            .map(|r| {
                self.github_repository_state(&r.repository)
                    .map(|s| s.status)
            })
            .collect::<Result<_>>()?;
        Ok(GithubWatcherStatus {
            revision: settings.revision,
            config: settings.config,
            repositories,
        })
    }
    /// Whole-config compare-and-swap. The daemon restricts this to operators.
    pub fn configure_github_watcher(
        &self,
        mut config: GithubWatcherConfig,
        expected_revision: u64,
    ) -> Result<GithubWatcherStatus> {
        config.validate().map_err(RuntimeError::Invalid)?;
        config.normalize();
        let hold = self.hold();
        let previous = self.github_settings()?;
        if previous.revision != expected_revision {
            return Err(RuntimeError::Invalid(
                "GitHub watcher configuration changed; refresh before editing".into(),
            ));
        }
        let mut commit = Commit::new();
        let at = now();
        for repo in &config.repositories {
            if matches!(repo.dispatch, GithubDispatchMode::Fixed) {
                self.check_fixed_target_live(repo, &config)?;
            }
            let previous_repo = previous
                .config
                .repositories
                .iter()
                .find(|p| p.repository == repo.repository);
            let reset = previous_repo.is_none()
                || (config.enabled
                    && repo.enabled
                    && (!previous.config.enabled || previous_repo.is_some_and(|p| !p.enabled)));
            if reset {
                let mut state = if self.store.meta(&state_key(&repo.repository))?.is_some() {
                    self.github_repository_state(&repo.repository)?
                } else {
                    RepositoryState {
                        baseline: at,
                        cursor: at,
                        review_cursor: at,
                        failures: 0,
                        status: GithubRepositoryStatus {
                            repository: repo.repository.clone(),
                            ..Default::default()
                        },
                    }
                };
                state.baseline = at;
                state.cursor = at;
                state.review_cursor = at;
                state.failures = 0;
                state.status.next_poll_at = Some(at);
                state.status.last_error = None;
                commit.push_mutation(encode(state_key(&repo.repository), &state)?);
            }
        }
        let settings = Settings {
            revision: previous
                .revision
                .checked_add(1)
                .ok_or_else(|| RuntimeError::Internal("GitHub revision overflow".into()))?,
            config,
        };
        commit.push_mutation(encode(CONFIG_KEY.into(), &settings)?);
        self.apply_held(hold, commit)?;
        self.github_watcher_status()
    }

    /// `Fixed` dispatch names one agent synchronously checkable now; `Thread`
    /// dispatch has nothing to check here; its template was already validated
    /// by `config.validate()` above, and each thread resolves its own target
    /// lazily as requests arrive.
    fn check_fixed_target_live(
        &self,
        repo: &GithubRepositoryConfig,
        config: &GithubWatcherConfig,
    ) -> Result<()> {
        debug_assert!(matches!(repo.dispatch, GithubDispatchMode::Fixed));
        let id = repo.agent.ok_or_else(|| {
            RuntimeError::Internal(format!(
                "GitHub repository {} uses fixed dispatch but names no agent; \
                 config.validate() should have refused this",
                repo.repository
            ))
        })?;
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        debug_assert_eq!(
            agent.id, id,
            "store returned a different agent than asked for"
        );
        if repo.enabled && config.enabled && (agent.paused || agent.status.is_terminal()) {
            return Err(RuntimeError::Invalid(format!(
                "GitHub target {id} is paused or terminal"
            )));
        }
        Ok(())
    }

    /// One supervised daemon worker. No process, network or model call while disabled.
    pub async fn run_github_watcher(self: Arc<Self>, shutdown: CancellationToken) {
        let source = source::Gh;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                result = self.github_poll(&source, now()) => {
                    if let Err(error) = result { tracing::error!(%error, "GitHub watcher failed; retrying in 5 seconds"); }
                }
            }
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    }

    async fn github_poll(&self, source: &impl source::Source, at: Timestamp) -> Result<()> {
        let settings = self.github_settings()?;
        if !settings.config.enabled {
            return Ok(());
        }
        if let Some(raw) = self.store.meta("github.watcher.rate_until")? {
            let until: Timestamp = raw.parse().map_err(|e| {
                RuntimeError::Internal(format!("corrupt GitHub rate deadline: {e}"))
            })?;
            if until > at {
                return Ok(());
            }
        }
        for repo in settings.config.repositories.iter().filter(|r| r.enabled) {
            let state = self.github_repository_state(&repo.repository)?;
            if state.status.next_poll_at.is_some_and(|t| t > at) {
                continue;
            }
            let outcome = poll::scan(self, source, &settings, repo, &state, at).await;
            let hold = self.hold();
            if self.github_settings()?.revision != settings.revision {
                return Ok(());
            }
            // Re-read: each successful decision has already committed its counters.
            let mut state = self.github_repository_state(&repo.repository)?;
            state.status.last_poll_at = Some(at);
            let mut rate_until = None;
            let delay = match outcome {
                Ok(()) => {
                    state.review_cursor = at;
                    state.status.last_success_at = Some(at);
                    state.status.last_error = None;
                    state.failures = 0;
                    settings.config.poll_interval_secs
                }
                Err(error) => {
                    tracing::warn!(repository = %repo.repository, %error, "GitHub repository poll failed");
                    if error.retry_after > 0 {
                        rate_until =
                            Some(now() + chrono::Duration::seconds(error.retry_after as i64));
                    }
                    state.status.last_error = Some(error.message);
                    state.failures = state.failures.saturating_add(1);
                    settings
                        .config
                        .poll_interval_secs
                        .saturating_mul(1u64 << state.failures.min(5))
                        .min(3600)
                        .max(settings.config.poll_interval_secs)
                        .max(error.retry_after)
                }
            };
            state.status.next_poll_at = Some(now() + chrono::Duration::seconds(delay as i64));
            let mut commit = Commit::new().mutate(encode(state_key(&repo.repository), &state)?);
            if let Some(until) = rate_until {
                commit.push_mutation(Mutation::SetMeta {
                    key: "github.watcher.rate_until".into(),
                    value: until.to_rfc3339(),
                });
            }
            self.apply_held(hold, commit)?;
            if rate_until.is_some() {
                break;
            }
        }
        Ok(())
    }

    fn github_advance_cursor(&self, revision: u64, repo: &str, at: Timestamp) -> Result<()> {
        let hold = self.hold();
        if self.github_settings()?.revision != revision {
            return Err(RuntimeError::Invalid(
                "GitHub settings changed during poll".into(),
            ));
        }
        let mut state = self.github_repository_state(repo)?;
        state.cursor = at;
        self.apply_held(hold, Commit::new().mutate(encode(state_key(repo), &state)?))?;
        Ok(())
    }

    /// Resolve a fixed or thread target ahead of the delivery hold, staging any
    /// replacement's creation without committing or starting it separately.
    fn github_target(&self, repo: &GithubRepositoryConfig, number: u64) -> Result<Target> {
        debug_assert!(
            !repo.repository.is_empty(),
            "repository is validated at config time"
        );
        debug_assert!(number > 0, "GitHub thread numbers are always positive");
        match &repo.dispatch {
            GithubDispatchMode::Fixed => {
                let id = repo.agent.ok_or_else(|| {
                    RuntimeError::Internal(format!(
                        "GitHub repository {} uses fixed dispatch but names no agent; \
                         config.validate() should have refused this",
                        repo.repository
                    ))
                })?;
                self.store
                    .agent(id)?
                    .map(|agent| Target::Existing(agent.id))
                    .ok_or_else(|| RuntimeError::NotFound(id.to_string()))
            }
            GithubDispatchMode::Thread(template) => {
                self.github_thread_target(repo, number, template)
            }
            GithubDispatchMode::Automatic => Err(RuntimeError::Internal(
                "automatic GitHub targets are prepared inside the delivery transaction".into(),
            )),
        }
    }

    async fn github_automatic_target(
        &self,
        repo: &GithubRepositoryConfig,
        number: u64,
    ) -> Result<Target> {
        let mapping = self
            .store
            .meta(&automatic_agent_key(&repo.repository, number))?;
        let generation: Option<u64> = self
            .store
            .meta(&automatic_generation_key(&repo.repository, number))?
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .map_err(|e| {
                RuntimeError::Internal(format!("corrupt GitHub checkout generation: {e}"))
            })?;
        if let Some(agent) = self.live_thread_agent(repo, number, true)? {
            let generation = generation.ok_or_else(|| {
                RuntimeError::Internal(
                    "automatic GitHub agent mapping has no checkout generation".into(),
                )
            })?;
            let workspace = checkout::verify(self, &repo.repository, number, generation).await?;
            // Resolved like the agent's own id was at creation, so an alias
            // registered for this repository does not orphan its thread agent.
            let repository = self
                .store
                .resolve_repository(&format!("github.com/{}", repo.repository))?;
            let scope = self.resolved_scope(&agent.scope)?;
            if scope.workspace_root.as_ref() != Some(&workspace)
                || scope.repository.as_deref() != Some(repository.as_str())
            {
                return Err(RuntimeError::Invalid(format!(
                    "GitHub agent {} no longer has its managed checkout scope; delivery retained for retry",
                    agent.id
                )));
            }
            return Ok(Target::Existing(agent.id));
        }
        let generation = if mapping.is_some() {
            generation
                .ok_or_else(|| {
                    RuntimeError::Internal(
                        "automatic GitHub agent mapping has no checkout generation".into(),
                    )
                })?
                .checked_add(1)
                .ok_or_else(|| {
                    RuntimeError::Internal("GitHub checkout generation overflow".into())
                })?
        } else {
            0
        };
        let workspace = checkout::prepare(self, &repo.repository, number, generation).await?;
        let scope = Scope {
            workspace_root: Some(workspace),
            repository: Some(format!("github.com/{}", repo.repository)),
            network: self.config.network_default,
            ..Default::default()
        };
        let prepared = self.prepare_agent(
            CreateAgentSpec {
                role: "github".into(),
                name: Some(format!("GitHub {} #{number}", repo.repository)),
                objective: Some(format!(
                    "Handle authorized requests in {repo} issue or PR #{number}. \
                     The GitHub trigger in your inbox contains the request. One marked \
                     [authenticated operator instruction] is from a verified operator \
                     login and carries the local operator's authority; any other is \
                     external content from a collaborator with write access. \
                     Work in this thread's checkout, preserve approval boundaries, and \
                     include <!-- aether:github-reply --> in any GitHub reply.",
                    repo = repo.repository
                )),
                scope: Some(scope),
                start: false,
                ..Default::default()
            },
            &Actor::System,
            Preflight::default(),
        )?;
        Ok(Target::Prepared {
            agent: Box::new(prepared),
            generation: Some(generation),
        })
    }

    /// Reuse a live mapped agent, or prepare its replacement for the delivery
    /// transaction. Preparation may run Git, so it must happen outside the hold.
    fn github_thread_target(
        &self,
        repo: &GithubRepositoryConfig,
        number: u64,
        template: &SpawnSpec,
    ) -> Result<Target> {
        if let Some(agent) = self.live_thread_agent(repo, number, false)? {
            return Ok(Target::Existing(agent.id));
        }
        let objective = format!(
            "{}\n\nGitHub thread: repository {}, issue/PR #{number}.",
            template.objective.trim(),
            repo.repository
        );
        debug_assert!(
            !objective.trim().is_empty(),
            "validate() requires a non-empty objective"
        );
        let prepared = self.prepare_agent(
            CreateAgentSpec {
                role: template.role.clone(),
                name: template.name.clone(),
                objective: Some(objective),
                model: template.model.clone(),
                fallbacks: template.fallbacks.clone(),
                scope: template.scope.clone(),
                repository_root: None,
                parent: None,
                config: template.config.clone(),
                approval: template.approval.clone(),
                run: None,
                start: false,
            },
            &Actor::System,
            Preflight::default(),
        )?;
        Ok(Target::Prepared {
            agent: Box::new(prepared),
            generation: None,
        })
    }

    /// `None` when no thread mapping exists yet, or the mapped agent is gone,
    /// paused, or terminal: either way the caller spawns a replacement.
    fn live_thread_agent(
        &self,
        repo: &GithubRepositoryConfig,
        number: u64,
        automatic: bool,
    ) -> Result<Option<Agent>> {
        let key = if automatic {
            automatic_agent_key(&repo.repository, number)
        } else {
            thread_agent_key(&repo.repository, number)
        };
        let Some(raw) = self.store.meta(&key)? else {
            return Ok(None);
        };
        let id: AgentId = raw.parse().map_err(|e| {
            RuntimeError::Internal(format!(
                "corrupt GitHub thread agent mapping for {}:{number}: {e}",
                repo.repository
            ))
        })?;
        debug_assert!(!raw.is_empty(), "SetMeta never stores an empty agent id");
        let Some(agent) = self.store.agent(id)? else {
            return Ok(None);
        };
        Ok((!agent.paused && !agent.status.is_terminal()).then_some(agent))
    }

    /// The settings revision and target liveness are checked again under the same
    /// hold as inbox sequencing; disabling while a request is in flight wins.
    async fn github_decide(
        &self,
        revision: u64,
        repo: &GithubRepositoryConfig,
        candidate: &poll::Candidate,
        allowed: bool,
        standing: &standing::Standing,
    ) -> Result<bool> {
        debug_assert!(
            candidate.number > 0,
            "GitHub thread numbers are always positive"
        );
        debug_assert!(
            allowed || !standing.is_operator(),
            "standing is decided only for a request that may dispatch"
        );
        let target = if allowed {
            Some(match repo.dispatch {
                GithubDispatchMode::Automatic => {
                    self.github_automatic_target(repo, candidate.number).await?
                }
                _ => self.github_target(repo, candidate.number)?,
            })
        } else {
            None
        };
        self.github_commit_decision(revision, repo, candidate, target, standing)
    }

    fn github_commit_decision(
        &self,
        revision: u64,
        repo: &GithubRepositoryConfig,
        candidate: &poll::Candidate,
        target: Option<Target>,
        standing: &standing::Standing,
    ) -> Result<bool> {
        let allowed = target.is_some();
        let hold = self.hold();
        if self.github_settings()?.revision != revision {
            return Err(RuntimeError::Invalid(
                "GitHub watcher settings changed during poll".into(),
            ));
        }
        let key = candidate.key(&repo.repository);
        if self.store.meta(&key)?.is_some() {
            return Ok(false);
        }
        let mut state = self.github_repository_state(&repo.repository)?;
        let mut commit = Commit::new();
        let cap_key = format!(
            "github.watcher.thread:{}:{}",
            repo.repository, candidate.number
        );
        let mut cap: (Timestamp, u32) = self
            .store
            .meta(&cap_key)?
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .map_err(|e| RuntimeError::Internal(format!("corrupt GitHub thread limit: {e}")))?
            .unwrap_or((now(), 0));
        if now() - cap.0 >= chrono::Duration::hours(1) {
            cap = (now(), 0);
        }
        if allowed && cap.1 >= 20 {
            return Err(RuntimeError::Invalid(format!(
                "GitHub thread {} hit dispatch cap (20/hour); request deferred",
                candidate.number
            )));
        }
        // A concurrent delivery may have committed a live thread agent while
        // this target was prepared. Reuse that winner and discard the preparation.
        let target = if allowed && matches!(repo.dispatch, GithubDispatchMode::Thread(_)) {
            match self.live_thread_agent(repo, candidate.number, false)? {
                Some(agent) => Some(Target::Existing(agent.id)),
                None => target,
            }
        } else {
            target
        };
        let mut created = None;
        let mut automatic_generation = None;
        if let Some(target) = target {
            // Liveness is re-checked here, under the hold, against the
            // resolved target's id rather than the snapshot from before it -
            // the same guarantee Fixed dispatch always had.
            let mut agent = match target {
                Target::Existing(id) => self
                    .store
                    .agent(id)?
                    .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?,
                Target::Prepared {
                    agent: prepared,
                    generation,
                } => {
                    let PreparedAgent {
                        agent,
                        commit: creation,
                        ..
                    } = *prepared;
                    commit.events.extend(creation.events);
                    commit.mutations.extend(creation.mutations);
                    created = Some(agent.clone());
                    automatic_generation = generation;
                    agent
                }
            };
            if agent.paused || agent.status.is_terminal() {
                return Err(RuntimeError::Invalid(format!(
                    "GitHub target {} is paused or terminal; delivery retained for retry",
                    agent.id
                )));
            }
            let trigger = InboxKind::ExternalTrigger {
                source: "github".into(),
                payload: standing::payload(&repo.repository, candidate, standing),
                operator: standing.is_operator(),
            };
            self.push_inbox(&hold, &mut commit, &agent, trigger, Actor::System)?;
            if created.is_some() {
                agent.transition(AgentStatus::Ready, now())?;
                commit.push_event(
                    EventDraft::new(Actor::System, EventKind::AgentReady)
                        .agent(agent.id)
                        .run(agent.run),
                );
                commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
                created = Some(agent.clone());
            }
            state.status.dispatched = state.status.dispatched.saturating_add(1);
            cap.1 += 1;
            commit.push_mutation(encode(cap_key, &cap)?);
            if matches!(
                repo.dispatch,
                GithubDispatchMode::Thread(_) | GithubDispatchMode::Automatic
            ) {
                // Same commit as the delivery: this candidate landing and this
                // thread now resuming through `agent` are one atomic fact.
                commit.push_mutation(Mutation::SetMeta {
                    key: if matches!(repo.dispatch, GithubDispatchMode::Automatic) {
                        automatic_agent_key(&repo.repository, candidate.number)
                    } else {
                        thread_agent_key(&repo.repository, candidate.number)
                    },
                    value: agent.id.to_string(),
                });
            }
            if let Some(generation) = automatic_generation {
                commit.push_mutation(encode(
                    automatic_generation_key(&repo.repository, candidate.number),
                    &generation,
                )?);
            }
        } else {
            state.status.ignored = state.status.ignored.saturating_add(1);
        }
        commit.push_mutation(Mutation::SetMeta {
            key,
            value: if allowed {
                "delivered"
            } else {
                "ignored: actor is a bot, not allowlisted, or lacks repository write access"
            }
            .into(),
        });
        commit.push_mutation(encode(state_key(&repo.repository), &state)?);
        self.apply_held(hold, commit)?;
        if let Some(agent) = created {
            self.seed_repository_knowledge(&agent, Actor::System);
        }
        Ok(true)
    }
}
