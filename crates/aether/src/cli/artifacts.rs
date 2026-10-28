use super::{
    ArtifactContents, ArtifactKind, ArtifactQuery, connect, io, sane, sane_document, short,
};
use anyhow::Context as _;
use std::io::Write as _;

pub async fn artifacts_list(
    socket: impl Into<aether_client::Endpoint>,
    agent: Option<super::AgentRef>,
    task: Option<super::TaskRef>,
    with_bytes: bool,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let agent = match agent {
        Some(agent) => Some(agent.resolve(&client, &socket).await?),
        None => None,
    };
    let task = match task {
        Some(task) => Some(task.resolve(&client).await?),
        None => None,
    };
    let rows = client
        .list_artifacts(ArtifactQuery {
            name: None,
            agent,
            task,
            with_bytes_only: with_bytes,
            limit,
        })
        .await?;
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    println!(
        "{:<32} {:<16} {:>10} {:<23} {:<32} summary",
        "id", "kind", "bytes", "created", "agent"
    );
    for a in rows {
        println!("{}", artifact_line(&a));
    }
    Ok(())
}

fn artifact_line(a: &aether_core::artifact::Artifact) -> String {
    format!(
        "{:<32} {:<16} {:>10} {:<23} {:<32} {}{}",
        a.id.to_string(),
        kind_label(&a.kind),
        a.size_bytes,
        a.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        a.agent_id.map(|id| id.to_string()).unwrap_or_default(),
        short(&a.summary, 60),
        if a.evicted_at.is_some() {
            " (bytes dropped by retention)"
        } else {
            ""
        },
    )
}

fn kind_label(kind: &ArtifactKind) -> String {
    match kind {
        ArtifactKind::Other { label } => sane(label),
        other => format!("{other:?}").to_lowercase(),
    }
}

pub(super) fn write_artifact_output<W, F>(out: &mut W, write: F) -> anyhow::Result<()>
where
    W: io::Write,
    F: FnOnce(&mut W) -> io::Result<()>,
{
    match write(out).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error).context("write artifact to stdout"),
    }
}

pub async fn artifact_show(
    socket: impl Into<aether_client::Endpoint>,
    id: String,
    max_bytes: usize,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let id = id.parse().context("invalid artifact id")?;
    let artifact = client.get_artifact(id).await?;
    if json {
        let json = serde_json::to_string(&artifact)?;
        let mut out = io::stdout().lock();
        return write_artifact_output(&mut out, |out| writeln!(out, "{json}"));
    }
    let contents = if let Some(at) = artifact.evicted_at.as_ref() {
        ArtifactContents::Dropped {
            at: at.to_rfc3339(),
        }
    } else {
        let (_, text, truncated) = client.artifact_bytes(id, max_bytes).await?;
        ArtifactContents::Retained { text, truncated }
    };
    let mut out = io::stdout().lock();
    write_artifact_output(&mut out, |out| {
        writeln!(out, "{} · {}", artifact.id, kind_label(&artifact.kind))?;
        writeln!(out, "name     {}", sane(&artifact.name))?;
        writeln!(out, "summary  {}", sane(&artifact.summary))?;
        writeln!(out, "size     {} bytes", artifact.size_bytes)?;
        writeln!(out, "created  {}", artifact.created_at.to_rfc3339())?;
        match contents {
            ArtifactContents::Dropped { ref at } => {
                writeln!(out, "bytes    dropped by retention at {at}")?;
            }
            ArtifactContents::Retained {
                ref text,
                truncated,
            } => {
                writeln!(out, "---")?;
                write!(out, "{}", sane_document(&String::from_utf8_lossy(text)))?;
                if truncated {
                    writeln!(out, "\n[truncated at {max_bytes} bytes]")?;
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn artifact_row_marks_created_time_as_utc() {
        let artifact = aether_core::artifact::Artifact {
            id: aether_core::id::ArtifactId::new(),
            kind: super::ArtifactKind::File,
            name: "result".into(),
            path: None,
            summary: "saved result".into(),
            size_bytes: 42,
            content_type: None,
            digest: None,
            created_by: aether_core::event::Actor::System,
            agent_id: None,
            task_id: None,
            created_at: "2026-01-02T03:04:05Z".parse().unwrap(),
            evicted_at: None,
        };
        let line = super::artifact_line(&artifact);
        assert!(line.contains("2026-01-02 03:04:05 UTC"), "{line}");
        assert!(line.ends_with("saved result"), "{line}");
    }
}
