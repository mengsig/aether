use super::connect;
use aether_core::retention::RunRetentionDecision;

pub async fn retention_preview(
    socket: impl Into<aether_client::Endpoint>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect(&socket).await?;
    let preview = client.preview_retention().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    println!(
        "Retention preview at {} (history cutoff {})",
        preview.at, preview.history_before
    );
    for run in &preview.runs {
        match &run.decision {
            RunRetentionDecision::Eligible => println!(
                "{} eligible: {} events, {} rows, {} payload bytes",
                run.run, run.events, run.rows, run.history_payload_bytes
            ),
            RunRetentionDecision::Protected { reason } => {
                println!("{} protected: {}", run.run, reason)
            }
        }
    }
    println!(
        "History: {} payload bytes reusable in SQLite; database file may not shrink",
        preview.history_payload_bytes
    );
    println!(
        "Artifacts: {} eligible records, {} payload bytes (shared blobs counted once)",
        preview.eligible_artifacts.len(),
        preview.artifact_payload_bytes
    );
    for artifact in &preview.eligible_artifacts {
        println!("{artifact} eligible artifact");
    }
    println!(
        "Pending cleanup: {} unlink records, {} staged publications (may still be in flight)",
        preview.pending_cleanup.pending_unlinks.len(),
        preview.pending_cleanup.staged_publications.len()
    );
    for artifact in &preview.pending_cleanup.pending_unlinks {
        println!("{artifact} pending unlink");
    }
    for artifact in &preview.pending_cleanup.staged_publications {
        println!("{artifact} staged publication");
    }
    Ok(())
}
