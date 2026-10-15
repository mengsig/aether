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

/// What `memory list` narrows by.
pub struct MemoryFilter {
    pub scope: Option<String>,
    pub text: Option<String>,
    pub tag: Vec<String>,
    pub recorded_by: Option<aether_core::memory::MemoryRecorder>,
    /// Include superseded and forgotten memories.
    pub all: bool,
    pub proposed: bool,
    pub limit: usize,
}

/// `operator`, named as the protocol names it so the two cannot drift.
pub fn parse_memory_recorder(text: &str) -> anyhow::Result<aether_core::memory::MemoryRecorder> {
    serde_json::from_value(serde_json::Value::String(text.trim().to_owned()))
        .map_err(|_| anyhow::anyhow!("`{text}` is not a recorder; expected `operator`"))
}

fn memory_query(filter: MemoryFilter) -> anyhow::Result<MemoryQuerySpec> {
    let scope = filter
        .scope
        .as_deref()
        .map(parse_memory_scope)
        .transpose()?;
    Ok(MemoryQuerySpec {
        provenance_agent: None,
        recorded_by: filter.recorded_by,
        scope,
        kinds: Vec::new(),
        tags: filter.tag,
        text: filter.text,
        include_superseded: filter.all,
        include_forgotten: filter.all,
        proposed: filter.proposed,
        id: None,
        limit: filter.limit,
    })
}

/// One word for where a memory stands, which is what a person scans for.
fn memory_state(m: &aether_core::memory::MemoryRecord) -> &'static str {
    if m.is_proposed() {
        return if m.forgotten_at.is_some() {
            "rejected"
        } else {
            "proposed"
        };
    }
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
    filter: MemoryFilter,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let unfiltered = filter.scope.is_none()
        && filter.text.is_none()
        && filter.tag.is_empty()
        && filter.recorded_by.is_none()
        && !filter.proposed;
    let all = filter.all;
    let query = memory_query(filter)?;
    let client = connect(&socket).await?;
    let memories = client.memories(query).await?;
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

async fn find_memory(
    client: &aether_client::Client,
    id: aether_core::id::MemoryId,
) -> anyhow::Result<aether_core::memory::MemoryRecord> {
    let mut query = memory_query(MemoryFilter {
        scope: None,
        text: None,
        tag: Vec::new(),
        recorded_by: None,
        all: true,
        proposed: false,
        limit: 1,
    })?;
    query.id = Some(id);
    let mut records = client.memories(query.clone()).await?;
    if records.is_empty()
        && client.welcome().is_some_and(|welcome| {
            welcome
                .features
                .iter()
                .any(|feature| feature == aether_protocol::features::MEMORY_PROPOSALS)
        })
    {
        query.proposed = true;
        records = match client.memories(query).await {
            Ok(records) => records,
            Err(aether_client::ClientError::Protocol(error))
                if error.code == aether_protocol::ErrorCode::Forbidden =>
            {
                return Err(anyhow::anyhow!("no memory {id}"));
            }
            Err(error) => return Err(error.into()),
        };
    }
    anyhow::ensure!(
        records.len() <= 1,
        "daemon returned several memories for {id}"
    );
    let record = records
        .pop()
        .ok_or_else(|| anyhow::anyhow!("no memory {id}"))?;
    anyhow::ensure!(
        record.id == id,
        "daemon returned a different memory for {id}"
    );
    Ok(record)
}

pub async fn memory_show(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id: aether_core::id::MemoryId = id.parse().context("invalid memory id")?;
    let record = find_memory(&client, id).await?;
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
    print_memory_provenance(&record.provenance);
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

fn print_memory_provenance(provenance: &aether_core::memory::MemoryProvenance) {
    if let Some(source) = &provenance.source {
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
    let origin = memory_origin(provenance);
    if !origin.is_empty() {
        println!("provenance {}", super::sane(&origin.join(" · ")));
    }
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
        if record.is_proposed() {
            println!("restored {id}; it awaits acceptance before recall");
        } else {
            println!("restored {id}; it is retrievable again");
        }
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

/// What an alias resolves to: a checkout's derived id, or an id named outright.
pub struct AliasTarget {
    pub root: Option<PathBuf>,
    pub repository: Option<String>,
}

pub async fn memory_alias(
    socket: impl Into<aether_client::Endpoint>,
    alias: String,
    target: AliasTarget,
    dry_run: bool,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        target.root.is_some() != target.repository.is_some(),
        "name exactly one of --root and --repository"
    );
    let socket = socket.into();
    let root = target.root.map(std::path::absolute).transpose()?;
    let client = connect(&socket).await?;
    let report = client
        .alias_repository(alias, root, target.repository, dry_run)
        .await?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let counts = &report.counts;
    let verb = if report.dry_run {
        "would move"
    } else {
        "moved"
    };
    println!(
        "{} -> {}: {verb} {} repository and {} repository-role memor{} ({} already held by the target)",
        super::sane(&report.alias),
        super::sane(&report.canonical),
        counts.repository,
        counts.repository_role,
        if counts.memories() == 1 { "y" } else { "ies" },
        counts.duplicates,
    );
    println!(
        "{} agent(s) name {}; their repository memory is read and written under {}",
        counts.agents,
        super::sane(&report.alias),
        super::sane(&report.canonical),
    );
    if report.dry_run {
        println!("dry run: nothing moved. Moved memories do not move back.");
    } else if report.existing {
        println!("the alias was already registered");
    }
    Ok(())
}

pub async fn memory_aliases(
    socket: impl Into<aether_client::Endpoint>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let aliases = client.repository_aliases().await?;
    if json {
        println!("{}", serde_json::to_string(&aliases)?);
        return Ok(());
    }
    if aliases.is_empty() {
        println!("no repository aliases.");
        return Ok(());
    }
    println!("{:<32} {:<48} created", "alias", "resolves to");
    for a in aliases {
        println!(
            "{:<32} {:<48} {}",
            short(&a.alias, 32),
            short(&a.canonical, 48),
            a.created_at.to_rfc3339()
        );
    }
    Ok(())
}
