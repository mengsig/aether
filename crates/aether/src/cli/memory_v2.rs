//! `aether memory explain|consolidate|export|import|migration-status`:
//! Memory 2.0's host commands (docs/memory/MEMORY-V2.md §5).

use std::path::PathBuf;

use aether_client::{Client, Endpoint};
use aether_core::id::InvocationId;
use aether_core::memory::MemoryScope;
use aether_core::memory_v2_reports::{
    MemoryConsolidation, MemoryExport, MemoryImport, MemoryMigrationStatus, RetrievalExplain,
};
use anyhow::Context as _;
use clap::Subcommand;

use super::{MemoryScopeRef, TaskRef, connect};

#[derive(Debug, Subcommand)]
pub enum MemoryV2Cmd {
    /// What retrieval did for each model call of a task, or one call.
    Explain {
        /// Every model call of this task, oldest first.
        #[arg(
            long,
            required_unless_present = "invocation",
            conflicts_with = "invocation"
        )]
        task: Option<TaskRef>,
        /// The one model call with this invocation id.
        #[arg(long)]
        invocation: Option<InvocationId>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Merge near-duplicate memories; --dry-run only plans it.
    Consolidate {
        /// Only this scope, as `memory list --scope` takes it; default every scope.
        #[arg(long)]
        scope: Option<MemoryScopeRef>,
        /// Report the clusters it would merge and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Write every memory of a scope, or the store, as a JSON Lines artifact.
    Export {
        /// Only this scope, as `memory list --scope` takes it.
        #[arg(long)]
        scope: Option<MemoryScopeRef>,
        /// Where to write the artifact; standard output by default.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Print the reply as JSON; with --output, only its count and digest.
        #[arg(long)]
        json: bool,
    },
    /// Insert what an export holds and this store lacks; never overwrites.
    Import {
        file: PathBuf,
        /// Report what would be inserted and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The store's Memory 2.0 migrations, counts and backups.
    MigrationStatus {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn memory_v2(socket: Endpoint, command: MemoryV2Cmd) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    match command {
        MemoryV2Cmd::Explain {
            task,
            invocation,
            json,
        } => {
            debug_assert!(
                task.is_some() != invocation.is_some(),
                "clap takes exactly one"
            );
            let task = match task {
                Some(task) => Some(task.resolve(&client).await?),
                None => None,
            };
            let explain = client.memory_retrieval_explain(task, invocation).await?;
            print_or(json, &explain, print_explain)
        }
        MemoryV2Cmd::Consolidate {
            scope,
            dry_run,
            json,
        } => {
            let scope = resolve(scope, &client, &socket).await?;
            let report = client.consolidate_memories(scope, dry_run).await?;
            print_or(json, &report, print_consolidation)
        }
        MemoryV2Cmd::Export {
            scope,
            output,
            json,
        } => {
            let scope = resolve(scope, &client, &socket).await?;
            let export = client.memory_export(scope).await?;
            write_export(&export, output, json)
        }
        MemoryV2Cmd::Import {
            file,
            dry_run,
            json,
        } => {
            let artifact = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            let report = client.memory_import(artifact, dry_run).await?;
            print_or(json, &report, print_import)
        }
        MemoryV2Cmd::MigrationStatus { json } => {
            let status = client.memory_migration_status().await?;
            print_or(json, &status, print_status)
        }
    }
}

/// The artifact goes to `output`, or to standard output; it is written
/// whole or the command fails, never half.
fn write_export(export: &MemoryExport, output: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        export.artifact.lines().count() == export.records as usize + 1,
        "the daemon's export holds {} lines for {} memories",
        export.artifact.lines().count(),
        export.records
    );
    let Some(path) = output else {
        if json {
            println!("{}", serde_json::to_string(export)?);
        } else {
            print!("{}", export.artifact);
        }
        return Ok(());
    };
    stage_and_publish(&path, &export.artifact)?;
    if json {
        let summary =
            serde_json::json!({"output": path, "records": export.records, "digest": export.digest});
        println!("{summary}");
    } else {
        eprintln!(
            "{} memories written to {} (sha256 {})",
            export.records,
            path.display(),
            export.digest
        );
    }
    Ok(())
}

/// Writes `artifact` to a staging file this call alone created, beside
/// `path`, then renames that file onto `path`: never a shared or followed path.
fn stage_and_publish(path: &std::path::Path, artifact: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .with_context(|| format!("{} names no file", path.display()))?;
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    };
    let mut staged = tempfile::Builder::new()
        .prefix(&format!(".{}.", name.to_string_lossy()))
        .suffix(".partial")
        .tempfile_in(directory)
        .with_context(|| format!("create a staging file in {}", directory.display()))?;
    staged
        .write_all(artifact.as_bytes())
        .and_then(|()| staged.as_file().sync_all())
        .with_context(|| format!("write {}", staged.path().display()))?;
    let staged_len = staged.as_file().metadata()?.len();
    anyhow::ensure!(
        staged_len == artifact.len() as u64,
        "the staging file holds {staged_len} of {} bytes",
        artifact.len()
    );
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("move the export to {}", path.display()))?;
    Ok(())
}

async fn resolve(
    scope: Option<MemoryScopeRef>,
    client: &Client,
    socket: &Endpoint,
) -> anyhow::Result<Option<MemoryScope>> {
    match scope {
        Some(scope) => Ok(Some(scope.resolve(client, socket).await?)),
        None => Ok(None),
    }
}

fn print_or<T: serde::Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce(&T),
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        human(value);
    }
    Ok(())
}

fn word<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

fn print_explain(explain: &RetrievalExplain) {
    if explain.turns.is_empty() {
        println!("no model calls recorded.");
    }
    for turn in &explain.turns {
        println!(
            "{} turn {} at {}",
            turn.snapshot,
            turn.turn,
            turn.at.to_rfc3339()
        );
        if !turn.traced {
            println!("  not traced (an older turn, or one without retrieval)");
            continue;
        }
        println!(
            "  pool {}, injected {}, omitted {} over limit and {} cut",
            turn.pool,
            turn.injected.len(),
            turn.omitted.over_limit,
            turn.omitted.cut
        );
        for c in &turn.candidates {
            let rank = c.rank.map_or_else(|| "-".into(), |rank| rank.to_string());
            let score = c
                .score
                .map_or_else(|| "-".into(), |score| format!("{score:.3}"));
            let flags = [(c.stale, " stale"), (c.pinned, " pinned")]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name)
                .collect::<String>();
            println!(
                "  {:>3} {} {:<10} score {score} relevance {:.3}{flags}",
                rank,
                c.memory,
                word(&c.outcome),
                c.relevance
            );
        }
    }
    if explain.truncated {
        println!("(only the first 256 model calls are listed)");
    }
}

fn print_consolidation(report: &MemoryConsolidation) {
    let verb = if report.dry_run {
        "planned"
    } else {
        "consolidated"
    };
    println!(
        "{} memories scanned; {} clusters {verb}",
        report.scanned,
        report.clusters.len()
    );
    for cluster in &report.clusters {
        let members: Vec<String> = cluster.members.iter().map(ToString::to_string).collect();
        println!(
            "  {} {} <- {}{}{}",
            word(&cluster.outcome),
            cluster.survivor,
            members.join(" "),
            if cluster.promoted { " (promoted)" } else { "" },
            cluster
                .reason
                .as_deref()
                .map(|r| format!(": {}", super::sane(r)))
                .unwrap_or_default()
        );
    }
}

fn print_import(report: &MemoryImport) {
    let verb = if report.dry_run {
        "would insert"
    } else {
        "inserted"
    };
    println!(
        "{verb} {} of {}; {} unchanged; {} conflicts",
        report.inserted,
        report.records,
        report.unchanged,
        report.conflicts.len()
    );
    for conflict in &report.conflicts {
        println!(
            "  conflict {}: {}",
            conflict.memory,
            super::sane(&conflict.reason)
        );
    }
}

fn print_status(status: &MemoryMigrationStatus) {
    println!("schema {}", status.schema_version);
    for migration in &status.migrations {
        let mark = if migration.applied {
            "applied"
        } else {
            "pending"
        };
        println!("  V{} {} ({mark})", migration.version, migration.name);
    }
    let m = &status.memories;
    println!(
        "memories {}: {} admitted, {} implicit, {} candidates, {} stale; {} holdouts logged",
        m.total, m.admitted, m.implicit, m.candidates, m.stale, m.held_out
    );
    for backup in &status.backups {
        println!(
            "  backup {} ({} -> {}, {} bytes)",
            backup.name, backup.from_version, backup.to_version, backup.bytes
        );
    }
    if let Some(last) = &status.last_import {
        println!(
            "last import {}: {} inserted, {} unchanged, {} conflicts (sha256 {})",
            last.at.to_rfc3339(),
            last.inserted,
            last.unchanged,
            last.conflicts,
            last.digest
        );
    }
}
