use super::{MemoryQuerySpec, PathBuf, connect, short};
use anyhow::Context as _;

/// `global`, `role:<role>`, `agent:<id>`, `subtree:<agent id>`,
/// `repo-role:<role>@<repository>`, or anything else as a repository.
/// Every scope a memory can carry has to be nameable, or the person curating
/// them cannot list what they are curating.
pub(super) fn parse_memory_scope(text: &str) -> anyhow::Result<aether_core::memory::MemoryScope> {
    use aether_core::memory::MemoryScope;
    let text = text.trim();
    if text.eq_ignore_ascii_case("global") {
        return Ok(MemoryScope::Global);
    }
    if let Some(rest) = text.strip_prefix("repo-role:") {
        // A role family never contains `@`, so the first one ends it.
        let (role, repository) = rest
            .split_once('@')
            .ok_or_else(|| anyhow::anyhow!("`{text}`: expected repo-role:<role>@<repository>"))?;
        let role = role.trim();
        if !aether_core::memory::is_role_family(role) {
            anyhow::bail!("`{role}` is not a role family a repository can file notes under");
        }
        let id = aether_core::repository::normalize_id(repository)
            .ok_or_else(|| anyhow::anyhow!("`{repository}` does not name a repository"))?;
        return Ok(MemoryScope::RepositoryRole {
            id,
            role: role.to_owned(),
        });
    }
    if let Some(root) = text.strip_prefix("subtree:") {
        return Ok(MemoryScope::Subtree {
            root: root.trim().parse()?,
        });
    }
    if let Some(role) = text.strip_prefix("role:") {
        return Ok(MemoryScope::Role {
            role: role.trim().to_owned(),
        });
    }
    if let Some(id) = text.strip_prefix("agent:") {
        return Ok(MemoryScope::Agent {
            id: id.trim().parse()?,
        });
    }
    let id = aether_core::repository::normalize_id(text.strip_prefix("repo:").unwrap_or(text))
        .ok_or_else(|| anyhow::anyhow!("`{text}` does not name a repository"))?;
    Ok(MemoryScope::Repository { id })
}

fn memory_query(
    scope: Option<aether_core::memory::MemoryScope>,
    text: Option<String>,
    tag: Vec<String>,
    all: bool,
    limit: usize,
) -> MemoryQuerySpec {
    MemoryQuerySpec {
        provenance_agent: None,
        scope,
        kinds: Vec::new(),
        tags: tag,
        text,
        include_superseded: all,
        include_forgotten: all,
        id: None,
        limit,
    }
}

/// One word for where a memory stands, which is what a person scans for.
fn memory_state(m: &aether_core::memory::MemoryRecord) -> &'static str {
    match (
        m.forgotten_at.is_some(),
        m.superseded_by.is_some(),
        m.pinned,
    ) {
        (true, _, _) => "forgotten",
        (_, true, _) => "superseded",
        (_, _, true) => "pinned",
        _ => "current",
    }
}

pub(super) fn memory_scope_label(scope: &aether_core::memory::MemoryScope) -> String {
    use aether_core::memory::MemoryScope;
    match scope {
        MemoryScope::Global => "global".into(),
        MemoryScope::Repository { id } => format!("repo {id}"),
        MemoryScope::Role { role } => format!("role {role}"),
        MemoryScope::Agent { id } => format!("agent {id}"),
        MemoryScope::RepositoryRole { id, role } => format!("repo-role {role}@{id}"),
        MemoryScope::Subtree { root } => format!("subtree {root}"),
    }
}

fn memory_provenance_label(provenance: &aether_core::memory::MemoryProvenance) -> String {
    let source = provenance.source.as_ref().map(|source| source.label());
    let sources: Vec<String> = source
        .into_iter()
        .chain(memory_origin(provenance))
        .collect();
    super::sane(&sources.join(" · "))
}

/// Who and what produced a memory, apart from the file it was imported from.
fn memory_origin(provenance: &aether_core::memory::MemoryProvenance) -> Vec<String> {
    let mut sources = Vec::new();
    if let Some(actor) = &provenance.actor {
        sources.push(actor.label());
    }
    if let Some(agent) = provenance.agent {
        sources.push(format!("agent {agent}"));
    }
    if let Some(task) = provenance.task {
        sources.push(format!("task {task}"));
    }
    if let Some(event) = provenance.event {
        sources.push(format!("event {event}"));
    }
    if let Some(artifact) = provenance.artifact {
        sources.push(format!("artifact {artifact}"));
    }
    sources
}

pub async fn memory_list(
    socket: impl Into<aether_client::Endpoint>,
    scope: Option<String>,
    text: Option<String>,
    tag: Vec<String>,
    all: bool,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let scope = scope.as_deref().map(parse_memory_scope).transpose()?;
    let unfiltered = scope.is_none() && text.is_none() && tag.is_empty();
    let client = connect(&socket).await?;
    let memories = client
        .memories(memory_query(scope, text, tag, all, limit))
        .await?;
    if json {
        println!("{}", serde_json::to_string(&memories)?);
        return Ok(());
    }
    if memories.is_empty() {
        println!("no memories match this query.");
        if !all {
            println!("Use --all to include forgotten and superseded memories.");
        }
        if unfiltered {
            println!(
                "Agents add memories with `remember`; `{} memory sync` imports a",
                super::command_hint(&socket, false)
            );
            println!("repository's committed notes.");
        }
        return Ok(());
    }
    println!(
        "{:<30} {:<10} {:<11} {:<18} {:<52} source",
        "id", "kind", "state", "scope", "text"
    );
    for m in memories {
        println!(
            "{:<30} {:<10} {:<11} {:<18} {:<52} {}",
            m.id.to_string(),
            serde_json::to_value(m.kind)?.as_str().unwrap_or("fact"),
            memory_state(&m),
            short(&memory_scope_label(&m.scope), 18),
            short(&m.text, 52),
            memory_provenance_label(&m.provenance),
        );
    }
    Ok(())
}

pub async fn memory_show(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id: aether_core::id::MemoryId = id.parse().context("invalid memory id")?;
    let one = |list: Vec<aether_core::memory::MemoryRecord>| list.into_iter().next();
    let mut query = memory_query(None, None, Vec::new(), true, 1);
    query.id = Some(id);
    let record =
        one(client.memories(query).await?).ok_or_else(|| anyhow::anyhow!("no memory {id}"))?;
    if json {
        println!("{}", serde_json::to_string(&record)?);
        return Ok(());
    }
    println!("{}  [{}]", record.id, memory_state(&record));
    println!("{}", super::sane(&record.text));
    println!();
    println!(
        "kind {} · scope {} · importance {:.2} · confidence {:.2}",
        serde_json::to_value(record.kind)?
            .as_str()
            .unwrap_or("fact"),
        super::sane(&memory_scope_label(&record.scope)),
        record.importance,
        record.confidence
    );
    if !record.tags.is_empty() {
        println!("tags {}", super::sane(&record.tags.join(", ")));
    }
    println!(
        "created {} · retrieved {} time(s){}",
        record.created_at.to_rfc3339(),
        record.retrieval_count,
        record
            .last_retrieved_at
            .map(|t| format!(", last {}", t.to_rfc3339()))
            .unwrap_or_default()
    );
    if let Some(source) = &record.provenance.source {
        println!(
            "from {}{}",
            super::sane(&source.file),
            source
                .commit
                .as_deref()
                .map(|c| format!(" at commit {}", super::sane(c)))
                .unwrap_or_default()
        );
    }
    let origin = memory_origin(&record.provenance);
    if !origin.is_empty() {
        println!("provenance {}", super::sane(&origin.join(" · ")));
    }
    for (label, id) in [
        ("supersedes", record.supersedes),
        ("superseded by", record.superseded_by),
    ] {
        if let Some(id) = id {
            println!("{label} {id}");
        }
    }
    if let Some(at) = record.forgotten_at {
        println!("forgotten {}", at.to_rfc3339());
    }
    Ok(())
}

pub async fn memory_forget(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    reason: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = id.parse().context("invalid memory id")?;
    let record = client.forget_memory(id, reason).await?;
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!("forgot {id}; the record stays in history and is never retrieved again");
    }
    Ok(())
}

pub async fn memory_restore(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = id.parse().context("invalid memory id")?;
    let record = client.restore_memory(id).await?;
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else {
        println!("restored {id}; it is retrievable again");
    }
    Ok(())
}

pub async fn memory_pin(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    unpin: bool,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = id.parse().context("invalid memory id")?;
    let record = client.pin_memory(id, !unpin).await?;
    if json {
        println!("{}", serde_json::to_string(&record)?);
    } else if record.pinned {
        println!("pinned {id}; it goes in front of the model on every turn in scope");
    } else {
        println!("unpinned {id}");
    }
    Ok(())
}

pub async fn memory_sync(
    socket: impl Into<aether_client::Endpoint>,
    root: Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let (repository, report) = client.sync_repository_knowledge(root).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "repository": repository,
                "report": report,
            }))?
        );
        return Ok(());
    }
    println!(
        "{}: {} note(s) read · {} added · {} replaced an older one · {} unchanged · {} retracted · {} not durable enough",
        super::sane(&repository),
        report.found,
        report.admitted,
        report.superseded,
        report.unchanged,
        report.retracted,
        report.rejected
    );
    if report.found == 0 {
        println!(
            "no notes found. Repository knowledge lives in committed markdown under {}/",
            aether_memory::repo::KNOWLEDGE_DIR
        );
    }
    Ok(())
}
