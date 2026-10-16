use std::fmt::Write as _;

use aether_core::workspace_review::{ChangedFile, FileStatus, WorkspaceDiff, WorkspaceIntegration};

use super::{AgentRef, acted, connect, sane, sane_document, short};

pub async fn workspace_diff(
    endpoint: impl Into<aether_client::Endpoint>,
    agent: AgentRef,
    patch: bool,
    json: bool,
) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = connect(&endpoint).await?;
    let id = agent.resolve(&client, &endpoint).await?;
    let diff = client.workspace_diff(id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
        return Ok(());
    }
    print!("{}", diff_summary(&diff));
    if patch && !diff.patch.is_empty() {
        println!();
        print!("{}", sane_document(&diff.patch));
    }
    Ok(())
}

fn file_line(file: &ChangedFile) -> String {
    let status = match file.status {
        FileStatus::Added => 'A',
        FileStatus::Modified => 'M',
        FileStatus::Deleted => 'D',
    };
    let lines = match (file.insertions, file.deletions) {
        (Some(added), Some(removed)) => format!("+{added} -{removed}"),
        _ => "binary".to_owned(),
    };
    format!("  {status} {}  {lines}\n", sane(&file.path))
}

/// The human view of a diff: where it is, what changed, and which commits.
fn diff_summary(diff: &WorkspaceDiff) -> String {
    debug_assert!(diff.files.len() <= aether_core::workspace_review::FILE_LIMIT);
    debug_assert!(
        diff.isolated != diff.ambiguous,
        "only shared diffs are ambiguous"
    );
    let mut out = String::new();
    let place = if diff.isolated {
        "isolated worktree"
    } else {
        "shared workspace"
    };
    let _ = writeln!(
        out,
        "{} {place} {}",
        diff.agent,
        sane(&diff.workspace.to_string_lossy())
    );
    if let Some(source) = &diff.source {
        let state = if diff.integrated { ", integrated" } else { "" };
        let _ = writeln!(out, "source {}{state}", sane(&source.to_string_lossy()));
    }
    if diff.ambiguous {
        out.push_str("shared: other agents' commits during these turns may be included\n");
    }
    let shown = if diff.files_truncated {
        " (first ones listed)"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "{} files changed{shown}, {} insertions(+), {} deletions(-)",
        diff.files.len(),
        diff.insertions,
        diff.deletions
    );
    for file in &diff.files {
        out.push_str(&file_line(file));
    }
    let older = if diff.commits_truncated {
        ", older ones omitted"
    } else {
        ""
    };
    let _ = writeln!(out, "{} commits{older}", diff.commits.len());
    for commit in &diff.commits {
        let turn = commit
            .turn
            .map(|turn| format!(" turn {turn}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {}{turn} {} ({})",
            commit.id.get(..12).unwrap_or(&commit.id),
            short(&commit.summary, 100),
            short(&commit.author, 40)
        );
    }
    if diff.patch_truncated {
        let _ = writeln!(out, "patch: {} bytes, cut for transport", diff.patch_bytes);
    }
    out
}

/// The exit code is 1 when conflicts refused the integration.
pub async fn workspace_integrate(
    endpoint: impl Into<aether_client::Endpoint>,
    agent: AgentRef,
    json: bool,
) -> anyhow::Result<i32> {
    let endpoint = endpoint.into();
    let client = connect(&endpoint).await?;
    let id = agent.resolve(&client, &endpoint).await?;
    let integration = client.workspace_integrate(id).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&integration)?);
    } else {
        print!("{}", integration_summary(&integration));
    }
    Ok(if integration.integrated { 0 } else { 1 })
}

fn integration_summary(integration: &WorkspaceIntegration) -> String {
    debug_assert!(integration.integrated == integration.conflicts.is_empty());
    let source = sane(&integration.source.to_string_lossy());
    let mut out = String::new();
    if !integration.integrated {
        let _ = writeln!(
            out,
            "integration refused: {source} changed paths {} also changed; nothing was written",
            integration.agent
        );
        for path in &integration.conflicts {
            let _ = writeln!(out, "  {}", sane(path));
        }
        return out;
    }
    if integration.already_integrated {
        let _ = writeln!(
            out,
            "{} was already integrated into {source}",
            integration.agent
        );
        return out;
    }
    let _ = writeln!(
        out,
        "integrated {} into {source}: {} paths changed",
        integration.agent,
        integration.changed_paths.len()
    );
    for path in &integration.changed_paths {
        let _ = writeln!(out, "  {}", sane(path));
    }
    out
}

pub async fn workspace_discard(
    endpoint: impl Into<aether_client::Endpoint>,
    agent: AgentRef,
    force: bool,
    json: bool,
) -> anyhow::Result<()> {
    let endpoint = endpoint.into();
    let client = connect(&endpoint).await?;
    let id = agent.resolve(&client, &endpoint).await?;
    let discard = client.workspace_discard(id, force).await?;
    let live = if discard.forced {
        " while it was live"
    } else {
        ""
    };
    acted(
        json,
        &format!(
            "discarded the workspace of {id}{live}: {}",
            discard.workspace.display()
        ),
        serde_json::to_value(&discard)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::id::AgentId;
    use aether_core::workspace_review::WorkspaceCommit;

    fn diff() -> WorkspaceDiff {
        WorkspaceDiff {
            agent: AgentId::new(),
            workspace: "/work/child".into(),
            source: Some("/work/repo".into()),
            isolated: true,
            ambiguous: false,
            integrated: false,
            files: vec![
                ChangedFile {
                    path: "src/lib.rs".into(),
                    status: FileStatus::Modified,
                    insertions: Some(3),
                    deletions: Some(1),
                },
                ChangedFile {
                    path: "logo.png".into(),
                    status: FileStatus::Added,
                    insertions: None,
                    deletions: None,
                },
            ],
            files_truncated: false,
            insertions: 3,
            deletions: 1,
            commits: vec![WorkspaceCommit {
                id: "0123456789abcdef0123456789abcdef01234567".into(),
                author: "Child".into(),
                at: "2026-10-16T12:00:00Z".parse().unwrap(),
                summary: "add logo\u{1b}[31m".into(),
                turn: Some(2),
            }],
            commits_truncated: false,
            patch: String::new(),
            patch_bytes: 0,
            patch_truncated: false,
        }
    }

    #[test]
    fn a_diff_summary_lists_files_diffstat_and_commits_by_turn() {
        let diff = diff();
        let summary = diff_summary(&diff);
        assert!(
            summary.contains("2 files changed, 3 insertions(+), 1 deletions(-)"),
            "{summary}"
        );
        assert!(summary.contains("  M src/lib.rs  +3 -1\n"), "{summary}");
        assert!(summary.contains("  A logo.png  binary\n"), "{summary}");
        assert!(
            summary.contains("  0123456789ab turn 2 add logo[31m (Child)"),
            "{summary}"
        );
        assert!(!summary.contains('\u{1b}'), "controls are stripped");
        assert!(!summary.contains("shared"), "{summary}");
    }

    #[test]
    fn a_refused_integration_lists_its_conflicts() {
        let agent = AgentId::new();
        let summary = integration_summary(&WorkspaceIntegration {
            agent,
            source: "/work/repo".into(),
            integrated: false,
            already_integrated: false,
            changed_paths: Vec::new(),
            conflicts: vec!["a.txt".into(), "b.txt".into()],
        });
        assert_eq!(
            summary,
            format!(
                "integration refused: /work/repo changed paths {agent} also changed; nothing was written\n  a.txt\n  b.txt\n"
            )
        );
    }
}
