//! Typeable references. Wherever the CLI expects an agent, run, task,
//! approval or schedule id it also takes a unique prefix of one, with or
//! without its `agt_`-style prefix, an exact name unique among live records,
//! `@last` and `@viewed`. Only the CLI resolves; agent-facing tools take
//! full ids.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use aether_client::{Client, Endpoint};
use aether_core::agent::{Agent, ArchiveTarget};
use aether_core::id::{AgentId, ApprovalId, RunId, ScheduleId, TaskId};
use aether_core::schedule::{Schedule, ScheduleStatus};
use aether_core::task::Task;
use aether_core::tool::ApprovalRequest;
use aether_protocol::{AgentQuery, TaskQuery};
use anyhow::{Context as _, ensure};

use super::{NotFoundError, UsageError};

/// The most recently created agent of the current run, or that run.
pub(crate) const LAST: &str = "@last";
/// The agent a TUI on this machine views, or its run.
pub(crate) const VIEWED: &str = "@viewed";
/// Candidates an ambiguity lists before it counts the rest.
const SHOWN: usize = 10;

/// What a reference names, for messages.
pub(crate) trait Kind {
    const NOUN: &'static str;
    /// The id prefix, for kinds a reference can name only by id. A kind
    /// with names (`None`) takes any text.
    const UNNAMED_PREFIX: Option<&'static str>;
}

#[derive(Debug, Clone)]
pub(crate) enum AgentKind {}
#[derive(Debug, Clone)]
pub(crate) enum RunKind {}
#[derive(Debug, Clone)]
pub(crate) enum TaskKind {}
#[derive(Debug, Clone)]
pub(crate) enum ApprovalKind {}
#[derive(Debug, Clone)]
pub(crate) enum ScheduleKind {}
/// `aether archive` takes an agent or a run.
#[derive(Debug, Clone)]
pub(crate) enum TargetKind {}

impl Kind for AgentKind {
    const NOUN: &'static str = "agent";
    const UNNAMED_PREFIX: Option<&'static str> = None;
}
impl Kind for RunKind {
    const NOUN: &'static str = "run";
    const UNNAMED_PREFIX: Option<&'static str> = Some(RunId::PREFIX);
}
impl Kind for TaskKind {
    const NOUN: &'static str = "task";
    const UNNAMED_PREFIX: Option<&'static str> = Some(TaskId::PREFIX);
}
impl Kind for ApprovalKind {
    const NOUN: &'static str = "approval";
    const UNNAMED_PREFIX: Option<&'static str> = Some(ApprovalId::PREFIX);
}
impl Kind for ScheduleKind {
    const NOUN: &'static str = "schedule";
    const UNNAMED_PREFIX: Option<&'static str> = None;
}
impl Kind for TargetKind {
    const NOUN: &'static str = "agent or run";
    const UNNAMED_PREFIX: Option<&'static str> = None;
}

/// What the operator typed where an id is expected, until a daemon's
/// records turn it into one. The only way from the text to an id is
/// `resolve`, so no command can skip the resolver.
pub(crate) struct Ref<K> {
    text: String,
    kind: PhantomData<fn() -> K>,
}

pub(crate) type AgentRef = Ref<AgentKind>;
pub(crate) type RunRef = Ref<RunKind>;
pub(crate) type TaskRef = Ref<TaskKind>;
pub(crate) type ApprovalRef = Ref<ApprovalKind>;
pub(crate) type ScheduleRef = Ref<ScheduleKind>;
pub(crate) type TargetRef = Ref<TargetKind>;

impl<K> Clone for Ref<K> {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            kind: PhantomData,
        }
    }
}

impl<K> fmt::Debug for Ref<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.text, f)
    }
}

impl<K> fmt::Display for Ref<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl<K: Kind> FromStr for Ref<K> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim();
        if text.is_empty() {
            return Err(format!("expected {} id, id prefix or name", K::NOUN));
        }
        if let Some(prefix) = K::UNNAMED_PREFIX
            && !text.starts_with('@')
            && !is_id_prefix(text, prefix)
        {
            return Err(format!(
                "expected {} id or id prefix ({prefix}_...)",
                K::NOUN
            ));
        }
        Ok(Self {
            text: text.to_owned(),
            kind: PhantomData,
        })
    }
}

impl<K> Ref<K> {
    /// The id itself when the text is a full one: it needs no lookup, and
    /// the daemon says whether it exists.
    fn exact<I: FromStr>(&self) -> Option<I> {
        self.text.parse().ok()
    }
}

impl AgentRef {
    /// `socket` is where `@viewed` looks for a TUI's view.
    pub(crate) async fn resolve(
        &self,
        client: &Client,
        socket: &Endpoint,
    ) -> anyhow::Result<AgentId> {
        if let Some(id) = self.exact() {
            return Ok(id);
        }
        if self.text == VIEWED {
            return viewed_agent(socket);
        }
        resolve_agent(&self.text, &all_agents(client).await?)
    }
}

impl RunRef {
    pub(crate) async fn resolve(
        &self,
        client: &Client,
        socket: &Endpoint,
    ) -> anyhow::Result<RunId> {
        if let Some(id) = self.exact() {
            return Ok(id);
        }
        let agents = all_agents(client).await?;
        if self.text == VIEWED {
            return run_of(viewed_agent(socket)?, &agents);
        }
        resolve_run(&self.text, &agents)
    }

    /// The id itself when the text is a full one, read with no daemon.
    pub(crate) fn full_id(&self) -> Option<RunId> {
        self.exact()
    }
}

impl TaskRef {
    pub(crate) async fn resolve(&self, client: &Client) -> anyhow::Result<TaskId> {
        let ids = Self::resolve_each(std::slice::from_ref(self), client).await?;
        ids.into_iter()
            .next()
            .context("one reference resolves to one task")
    }

    /// Several at once, listing the tasks at most once.
    pub(crate) async fn resolve_each(
        refs: &[Self],
        client: &Client,
    ) -> anyhow::Result<Vec<TaskId>> {
        let mut tasks: Option<Vec<Task>> = None;
        let mut ids = Vec::with_capacity(refs.len());
        for task in refs {
            if let Some(id) = task.exact() {
                ids.push(id);
                continue;
            }
            if tasks.is_none() {
                let listed = client.list_tasks(TaskQuery::default()).await;
                tasks = Some(listed.context("listing tasks to resolve a reference")?);
            }
            let listed = tasks.as_deref().context("tasks were just listed")?;
            ids.push(resolve_task(&task.text, listed)?);
        }
        ensure!(ids.len() == refs.len(), "every task reference resolves");
        Ok(ids)
    }
}

impl ApprovalRef {
    pub(crate) async fn resolve(&self, client: &Client) -> anyhow::Result<ApprovalId> {
        if let Some(id) = self.exact() {
            return Ok(id);
        }
        let pending = client.list_approvals().await;
        resolve_approval(
            &self.text,
            &pending.context("listing approvals to resolve a reference")?,
        )
    }
}

impl ScheduleRef {
    pub(crate) async fn resolve(&self, client: &Client) -> anyhow::Result<ScheduleId> {
        if let Some(id) = self.exact() {
            return Ok(id);
        }
        let jobs = client.schedules(None).await;
        resolve_schedule(
            &self.text,
            &jobs.context("listing schedules to resolve a reference")?,
        )
    }
}

impl TargetRef {
    pub(crate) async fn resolve(
        &self,
        client: &Client,
        socket: &Endpoint,
    ) -> anyhow::Result<ArchiveTarget> {
        if let Some(id) = self.exact::<AgentId>() {
            return Ok(ArchiveTarget::Agent(id));
        }
        if let Some(id) = self.exact::<RunId>() {
            return Ok(ArchiveTarget::Run(id));
        }
        if self.text == VIEWED {
            return viewed_agent(socket).map(ArchiveTarget::Agent);
        }
        resolve_target(&self.text, &all_agents(client).await?)
    }
}

/// What a TUI on this machine, talking to the daemon at `socket`, views.
pub(crate) fn viewed_agent(socket: &Endpoint) -> anyhow::Result<AgentId> {
    let socket = socket.local_path().with_context(|| {
        format!("`{VIEWED}` names what a TUI on this machine views, so it needs a local daemon")
    })?;
    let path = aether_tui::viewed::viewed_dir(socket);
    let viewed =
        aether_tui::viewed::read(&path).with_context(|| format!("looking up `{VIEWED}`"))?;
    viewed.with_context(|| {
        format!(
            "no running TUI views an agent, so `{VIEWED}` names none; \
             open `aether tui`, or give an id or a name"
        )
    })
}

fn run_of(agent: AgentId, agents: &[Agent]) -> anyhow::Result<RunId> {
    let found = agents.iter().find(|a| a.id == agent);
    let found = found.with_context(|| format!("the viewed agent {agent} is not on this daemon"))?;
    debug_assert_eq!(found.id, agent);
    Ok(found.run)
}

/// Every agent the daemon holds, cleared ones included: a finished agent is
/// still something to replay or report on.
async fn all_agents(client: &Client) -> anyhow::Result<Vec<Agent>> {
    let query = AgentQuery {
        include_archived: true,
        ..Default::default()
    };
    let agents = client.list_agents(query).await;
    agents.context("listing agents to resolve a reference")
}

/// One record a reference may name.
struct Candidate<'a, T> {
    id: String,
    /// Set only for live records: a finished one's name is free to reuse.
    name: Option<&'a str>,
    summary: String,
    value: T,
}

fn agent_candidate(agent: &Agent) -> Candidate<'_, AgentId> {
    Candidate {
        id: agent.id.to_string(),
        name: agent.is_live().then_some(agent.name.as_str()),
        summary: format!(
            "{} · {}",
            super::short(&agent.name, 24),
            agent.status.kind()
        ),
        value: agent.id,
    }
}

/// Each run once, described by its first root agent.
fn run_candidates(agents: &[Agent]) -> Vec<Candidate<'_, RunId>> {
    let mut roots: BTreeMap<RunId, &Agent> = BTreeMap::new();
    for agent in agents.iter().filter(|a| a.parent.is_none()) {
        let root = roots.entry(agent.run).or_insert(agent);
        if (agent.created_at, agent.id) < (root.created_at, root.id) {
            *root = agent;
        }
    }
    roots
        .into_iter()
        .map(|(run, root)| Candidate {
            id: run.to_string(),
            name: None,
            summary: format!(
                "{} · started {}",
                super::short(&root.name, 24),
                root.created_at.format("%Y-%m-%d %H:%M")
            ),
            value: run,
        })
        .collect()
}

/// The run whose root agent was created last.
fn current_run(agents: &[Agent]) -> anyhow::Result<RunId> {
    let newest_root = agents
        .iter()
        .filter(|a| a.parent.is_none())
        .max_by_key(|a| (a.created_at, a.id));
    let root = newest_root.with_context(|| format!("there is no run yet for `{LAST}` to name"))?;
    Ok(root.run)
}

pub(crate) fn resolve_agent(text: &str, agents: &[Agent]) -> anyhow::Result<AgentId> {
    if text == LAST {
        let run = current_run(agents)?;
        let newest = agents
            .iter()
            .filter(|a| a.run == run)
            .max_by_key(|a| (a.created_at, a.id))
            .context("the current run has its root agent")?;
        debug_assert_eq!(newest.run, run);
        return Ok(newest.id);
    }
    let candidates = agents.iter().map(agent_candidate).collect();
    pick(text, AgentKind::NOUN, "aether agents --all", candidates)
}

pub(crate) fn resolve_run(text: &str, agents: &[Agent]) -> anyhow::Result<RunId> {
    if text == LAST {
        return current_run(agents);
    }
    pick(
        text,
        RunKind::NOUN,
        "aether runs list",
        run_candidates(agents),
    )
}

pub(crate) fn resolve_task(text: &str, tasks: &[Task]) -> anyhow::Result<TaskId> {
    let candidates = tasks
        .iter()
        .map(|task| Candidate {
            id: task.id.to_string(),
            name: None,
            summary: format!(
                "{} · {}",
                task.status.kind().as_str(),
                super::short(&task.objective, 48)
            ),
            value: task.id,
        })
        .collect();
    pick(text, TaskKind::NOUN, "aether tasks", candidates)
}

/// Only pending approvals can be answered, so only they are candidates.
pub(crate) fn resolve_approval(
    text: &str,
    pending: &[ApprovalRequest],
) -> anyhow::Result<ApprovalId> {
    let candidates = pending
        .iter()
        .map(|request| Candidate {
            id: request.id.to_string(),
            name: None,
            summary: format!("{} · {}", request.agent_id, super::short(&request.tool, 24)),
            value: request.id,
        })
        .collect();
    pick(text, ApprovalKind::NOUN, "aether approvals", candidates)
}

pub(crate) fn resolve_schedule(text: &str, jobs: &[Schedule]) -> anyhow::Result<ScheduleId> {
    let is_live = |job: &Schedule| {
        matches!(
            job.status,
            ScheduleStatus::Active { .. } | ScheduleStatus::Paused { .. }
        )
    };
    // A finished schedule keeps its history, so its name finds it unless a
    // live one holds the name now.
    let live_holds_name = jobs.iter().any(|job| is_live(job) && job.name == text);
    let candidates = jobs
        .iter()
        .map(|job| Candidate {
            id: job.id.to_string(),
            name: (!live_holds_name || is_live(job)).then_some(job.name.as_str()),
            summary: super::short(&job.name, 32),
            value: job.id,
        })
        .collect();
    pick(text, ScheduleKind::NOUN, "aether schedule list", candidates)
}

/// An agent first for `@last`; otherwise agents and runs compete alike.
pub(crate) fn resolve_target(text: &str, agents: &[Agent]) -> anyhow::Result<ArchiveTarget> {
    if text == LAST {
        return resolve_agent(text, agents).map(ArchiveTarget::Agent);
    }
    let agent_targets = agents.iter().map(|agent| {
        let c = agent_candidate(agent);
        Candidate {
            value: ArchiveTarget::Agent(c.value),
            id: c.id,
            name: c.name,
            summary: c.summary,
        }
    });
    let run_targets = run_candidates(agents).into_iter().map(|c| Candidate {
        value: ArchiveTarget::Run(c.value),
        id: c.id,
        name: c.name,
        summary: c.summary,
    });
    let candidates = agent_targets.chain(run_targets).collect();
    pick(text, TargetKind::NOUN, "aether agents --all", candidates)
}

/// The one candidate whose id starts with `text` or whose live name is
/// `text`. Never a guess: none or several is an error that says which.
fn pick<T>(
    text: &str,
    noun: &str,
    listed_by: &str,
    candidates: Vec<Candidate<'_, T>>,
) -> anyhow::Result<T> {
    assert!(!text.is_empty(), "references are never empty");
    if text.starts_with('@') {
        return Err(anyhow::Error::new(UsageError(format!(
            "`{text}` is not a reference the CLI knows; `{LAST}` and `{VIEWED}` name an agent or a run"
        ))));
    }
    let mut matches: Vec<_> = candidates
        .into_iter()
        .filter(|c| c.name == Some(text) || id_starts_with(&c.id, text))
        .collect();
    if matches.len() == 1 {
        return Ok(matches.remove(0).value);
    }
    if matches.is_empty() {
        return Err(anyhow::Error::new(NotFoundError(format!(
            "no {noun} matches `{text}`; `{listed_by}` lists them (names match live ones only)"
        ))));
    }
    // Newest first: ids are ULIDs, so the text sorts by creation time.
    matches.sort_by(|a, b| b.id.cmp(&a.id));
    let mut message = format!(
        "`{text}` matches {} {noun}s; give more of the id:",
        matches.len()
    );
    for c in matches.iter().take(SHOWN) {
        message.push_str(&format!("\n  {}  {}", c.id, c.summary));
    }
    if matches.len() > SHOWN {
        message.push_str(&format!("\n  and {} more", matches.len() - SHOWN));
    }
    Err(anyhow::Error::new(UsageError(message)))
}

/// Whether `text` can begin an id with this prefix: Crockford base32, the
/// alphabet ULIDs are written in, with or without `prefix_` in front. A
/// ULID's first character is 0 to 7, the top of its 48-bit timestamp.
fn is_id_prefix(text: &str, prefix: &str) -> bool {
    const ULID_LEN: usize = 26;
    let ulid = match text.split_once('_') {
        Some((head, rest)) if head.eq_ignore_ascii_case(prefix) => rest,
        Some(_) => return false,
        None => text,
    };
    let crockford = |b: u8| b.is_ascii_alphanumeric() && !b"ILOU".contains(&b.to_ascii_uppercase());
    ulid.len() <= ULID_LEN
        && ulid
            .bytes()
            .next()
            .is_some_and(|b| (b'0'..=b'7').contains(&b))
        && ulid.bytes().all(crockford)
}

/// `agt_01J5`, `AGT_01j5` and `01j5` are all prefixes of `agt_01J5...`.
fn id_starts_with(id: &str, text: &str) -> bool {
    let unprefixed = id.split_once('_').map_or(id, |(_, ulid)| ulid);
    let haystack = if text.contains('_') { id } else { unprefixed };
    haystack
        .get(..text.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(text))
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
