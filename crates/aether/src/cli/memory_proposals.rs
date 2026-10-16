use super::AgentRef;
use super::memory::MemoryScopeRef;
use aether_client::{Client, Endpoint};
use aether_core::memory::{MemoryKind, MemoryScope, OperatorMemoryScope, OperatorMemorySpec};
use anyhow::Context as _;

#[derive(Debug, clap::Args)]
pub struct MemoryAddArgs {
    text: String,
    /// global, agent:<agent>, role:<role>, repo-role:<role>@<repository>, or a repository id.
    #[arg(long, default_value = "global", conflicts_with = "root")]
    scope: OperatorScopeRef,
    /// File under the repository derived from this checkout.
    #[arg(long)]
    root: Option<std::path::PathBuf>,
    #[arg(long, default_value = "fact", value_parser = parse_kind)]
    kind: MemoryKind,
    #[arg(long)]
    importance: Option<f32>,
    #[arg(long)]
    confidence: Option<f32>,
    #[arg(long)]
    tag: Vec<String>,
    #[arg(long)]
    supersedes: Option<aether_core::id::MemoryId>,
    /// Keep out of recall until an operator accepts it.
    #[arg(long)]
    proposed: bool,
}

fn parse_kind(text: &str) -> anyhow::Result<MemoryKind> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .context("expected fact, decision, lesson, constraint or preference")
}

impl MemoryAddArgs {
    /// Everything that needs no daemon, checked before connecting.
    fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.text.trim().is_empty(), "text is empty");
        for value in [self.importance, self.confidence].into_iter().flatten() {
            anyhow::ensure!(
                (0.0..=1.0).contains(&value),
                "scores must be between 0 and 1"
            );
        }
        Ok(())
    }

    async fn scope(
        &self,
        client: &Client,
        socket: &Endpoint,
    ) -> anyhow::Result<OperatorMemoryScope> {
        if let Some(root) = &self.root {
            return Ok(OperatorMemoryScope::Repository {
                root: Some(std::path::absolute(root)?),
                repository: None,
            });
        }
        match &self.scope {
            OperatorScopeRef::Ready(scope) => Ok(scope.clone()),
            OperatorScopeRef::Agent(agent) => Ok(OperatorMemoryScope::Agent {
                id: agent.resolve(client, socket).await?,
            }),
        }
    }

    fn into_spec(self, scope: OperatorMemoryScope) -> anyhow::Result<OperatorMemorySpec> {
        self.check()?;
        debug_assert!(self.importance.is_none_or(f32::is_finite));
        debug_assert!(self.confidence.is_none_or(f32::is_finite));
        Ok(OperatorMemorySpec {
            text: self.text,
            kind: self.kind,
            scope,
            importance: self.importance,
            confidence: self.confidence,
            tags: self.tag,
            supersedes: self.supersedes,
            proposed: self.proposed,
        })
    }
}

/// An operator memory's scope as typed; only an agent in it needs the daemon.
#[derive(Debug, Clone)]
pub(crate) enum OperatorScopeRef {
    Ready(OperatorMemoryScope),
    Agent(AgentRef),
}

impl std::str::FromStr for OperatorScopeRef {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        operator_scope(text).map_err(|error| format!("{error:#}"))
    }
}

fn operator_scope(text: &str) -> anyhow::Result<OperatorScopeRef> {
    if let Some(rest) = text.trim().strip_prefix("repo-role:") {
        let (role, repository) = rest
            .split_once('@')
            .context("expected repo-role:<role>@<repository>")?;
        anyhow::ensure!(!role.trim().is_empty(), "role is empty");
        let repository = aether_core::repository::normalize_id(repository)
            .context("scope does not name a repository")?;
        return Ok(OperatorScopeRef::Ready(
            OperatorMemoryScope::RepositoryRole {
                root: None,
                repository: Some(repository),
                role: role.trim().to_owned(),
            },
        ));
    }
    let fixed = match super::memory::parse_memory_scope(text)? {
        MemoryScopeRef::Agent(agent) => return Ok(OperatorScopeRef::Agent(agent)),
        MemoryScopeRef::Subtree(_) => {
            anyhow::bail!("operator memory creation does not support subtree scope")
        }
        MemoryScopeRef::Fixed(fixed) => fixed,
    };
    Ok(OperatorScopeRef::Ready(match fixed {
        MemoryScope::Global => OperatorMemoryScope::Global,
        MemoryScope::Agent { id } => OperatorMemoryScope::Agent { id },
        MemoryScope::Role { role } => OperatorMemoryScope::Role { role },
        MemoryScope::Repository { id } => OperatorMemoryScope::Repository {
            root: None,
            repository: Some(id),
        },
        MemoryScope::RepositoryRole { id, role } => OperatorMemoryScope::RepositoryRole {
            root: None,
            repository: Some(id),
            role,
        },
        MemoryScope::Subtree { .. } => {
            anyhow::bail!("operator memory creation does not support subtree scope")
        }
    }))
}

pub async fn memory_add(
    socket: impl Into<aether_client::Endpoint>,
    memory: MemoryAddArgs,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    memory.check()?;
    let client = super::connect(&socket).await?;
    let scope = memory.scope(&client, &socket).await?;
    let spec = memory.into_spec(scope)?;
    let proposed = spec.proposed;
    let text = spec.text.trim().to_owned();
    let record = client.create_memory(spec).await?;
    anyhow::ensure!(
        record.is_proposed() == proposed,
        "daemon returned an unexpected memory state"
    );
    anyhow::ensure!(record.text == text, "daemon returned different memory text");
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!(
            "{} {}",
            if record.is_proposed() {
                "proposed"
            } else {
                "recorded"
            },
            record.id
        );
    }
    Ok(())
}

pub async fn memory_accept(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    reason: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let id = id.parse().context("invalid memory id")?;
    let client = super::connect(socket).await?;
    let record = client.accept_memory(id, reason).await?;
    anyhow::ensure!(record.id == id, "daemon returned a different memory");
    anyhow::ensure!(record.is_current(), "daemon returned an inactive memory");
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!("accepted {id}; it is available for recall");
    }
    Ok(())
}

pub async fn memory_reject(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    reason: String,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(!reason.trim().is_empty(), "reason must not be blank");
    let id = id.parse().context("invalid memory id")?;
    let client = super::connect(socket).await?;
    let record = client.reject_memory(id, reason).await?;
    anyhow::ensure!(record.id == id, "daemon returned a different memory");
    anyhow::ensure!(
        record.is_proposed() && record.forgotten_at.is_some(),
        "daemon did not reject the proposal"
    );
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!("rejected {id}; it stays in history and is excluded from recall");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_scope_keeps_role_names_that_the_operator_protocol_accepts() {
        for role in ["reviewer.security", "archon-stage:default/review"] {
            let scope = operator_scope(&format!("repo-role:{role}@aether")).unwrap();
            let OperatorScopeRef::Ready(scope) = scope else {
                panic!("a repository role needs no daemon");
            };
            assert_eq!(
                scope,
                OperatorMemoryScope::RepositoryRole {
                    root: None,
                    repository: Some("aether".into()),
                    role: role.into(),
                }
            );
        }
        assert!(operator_scope("repo-role:@aether").is_err());
        assert!(operator_scope("repo-role:reviewer").is_err());
        assert!(operator_scope("subtree:planner").is_err());
        assert!(matches!(
            operator_scope("agent:planner").unwrap(),
            OperatorScopeRef::Agent(agent) if agent.to_string() == "planner"
        ));
    }
}
