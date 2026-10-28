use super::{invalid, same_run};
use crate::{Result, RuntimeError};
use aether_core::{
    event::EventKind,
    id::RunId,
    time::now,
    transfer::{EXPORT_FORMAT, Manifest, Record},
};
use aether_store::StoreError;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use sha2::Digest as _;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use aether_core::{checkpoint::Checkpoint, transfer::ExportSummary};
use aether_store::{AgentFilter, EventFilter, Store, TaskFilter};
use std::io::{BufWriter, Write as _};

/// Write a run's events, snapshots, checkpoints and artifacts to one file.
///
/// A run with a live agent is refused: exporting one would capture a
/// half-finished agent whose engine session and workspace live on this
/// machine, and importing that elsewhere would be a lie about its state.
///
/// Every record is masked by `redactor`, which must be the runtime's own so
/// the daemon's known secret values are masked along with the secret shapes.
pub fn export_run(
    store: &dyn Store,
    run: RunId,
    path: &Path,
    redactor: &aether_tools::Redactor,
) -> Result<ExportSummary> {
    let agents = store.agents(&AgentFilter {
        run: Some(run),
        ..Default::default()
    })?;
    if agents.is_empty() {
        return Err(RuntimeError::NotFound(format!("run {run}")));
    }
    let live: Vec<String> = agents
        .iter()
        .filter(|a| a.is_live())
        .map(|a| format!("{} ({})", a.name, a.id))
        .collect();
    if !live.is_empty() {
        return Err(invalid(format!(
            "run {run} still has live agents: {}. Let them finish, or cancel them, then export",
            live.join(", ")
        )));
    }
    let tasks = store.tasks(&TaskFilter {
        run: Some(run),
        ..Default::default()
    })?;

    // Written beside the target and renamed onto it on success, so the
    // caller's existing file is only ever replaced by a complete one. A
    // failure leaves the `.partial` where it is: named as what it is, and
    // there to be looked at, rather than a truncated export.
    // A stale partial must be inspected and removed explicitly before retrying.
    let partial = partial_path(path);
    // Every model message, tool input and artifact payload in the run lands
    // in this file, at a path the caller chose. It is not left to the umask.
    let file = aether_core::private_file::create_new(&partial).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            StoreError::Io(std::io::Error::new(
                error.kind(),
                format!(
                    "export partial {} already exists; inspect or remove it before retrying: {error}",
                    partial.display()
                ),
            ))
        } else {
            StoreError::Io(error)
        }
    })?;
    let mut out = BufWriter::new(file);
    let mut summary = ExportSummary {
        run,
        path: path.to_path_buf(),
        agents: agents.len() as u64,
        tasks: tasks.len() as u64,
        events: 0,
        snapshots: 0,
        invocations: 0,
        checkpoints: 0,
        artifacts: 0,
        artifact_payloads: 0,
        bytes_written: 0,
    };
    let mut written = 0u64;
    // Counted before the manifest is written, so the header can promise how
    // many events follow and an import can hold the file to it. The stream
    // below stops at the same sequence: the scheduler's artifact sweep runs
    // concurrently and appends `artifact.evicted` to this very run, so
    // without the pin the file would carry more events than it promised and
    // be refused on the far side.
    let (events_in_run, last_seq) = count_events(store, run)?;
    let manifest = Manifest {
        aether_export: EXPORT_FORMAT,
        source_instance: store.instance_id()?,
        source_run: run,
        exported_at: now(),
        binary_version: env!("CARGO_PKG_VERSION").to_owned(),
        store_schema: aether_store::SCHEMA_VERSION,
        events: events_in_run,
    };
    let header = serde_json::to_value(&manifest).map_err(StoreError::Serde)?;
    refuse_if_masked(redactor, header)?;
    written += write_line(&mut out, &manifest)?;
    for agent in &agents {
        let mut agent = agent.clone();
        // A handle to a live engine conversation on this machine, and the
        // counter reading billing measures that conversation against. Neither
        // has any meaning off this machine, and an imported baseline is a
        // file-authorable one.
        agent.engine_session = None;
        agent.engine_usage = None;
        agent.engine_sent = None;
        written += write_record(&mut out, redactor, &Record::Agent(agent))?;
    }
    for task in &tasks {
        written += write_record(&mut out, redactor, &Record::Task(task.clone()))?;
    }

    for agent in &agents {
        // Only delivery schedules name a target, so spawn schedules stay home.
        for job in store.schedules(Some(agent.id))? {
            let Some(source) = job.run() else {
                continue;
            };
            same_run(path, "schedule", source, run)?;
            written += write_record(&mut out, redactor, &Record::Schedule(job.clone()))?;
            for occurrence in store.schedule_history(job.id, usize::MAX)? {
                written +=
                    write_record(&mut out, redactor, &Record::ScheduleOccurrence(occurrence))?;
            }
        }
    }

    // Everything else the run produced is reachable from its events, so the
    // log decides what travels - there is no second list to fall behind it.
    let mut seen_snapshots = BTreeMap::new();
    let mut seen_invocations = BTreeMap::new();
    let mut seen_checkpoints = BTreeMap::new();
    let mut seen_artifacts = BTreeMap::new();
    let filter = EventFilter::for_run(run);
    let mut after = 0u64;
    'pages: loop {
        let page = store.events(after, 1_000, &filter)?;
        let Some(last) = page.last() else { break };
        let count = page.len();
        after = last.seq;
        for mut event in page {
            if event.seq > last_seq {
                break 'pages;
            }
            for id in snapshot_ids(&event.kind) {
                if seen_snapshots.insert(id, ()).is_none()
                    && let Some(snapshot) = store.snapshot(id)?
                {
                    written += write_record(&mut out, redactor, &Record::Snapshot(snapshot))?;
                    summary.snapshots += 1;
                }
            }
            if let EventKind::InvocationCompleted { invocation, .. } = &event.kind
                && seen_invocations.insert(*invocation, ()).is_none()
                && let Some(record) = store.invocation(*invocation)?
            {
                written += write_record(&mut out, redactor, &Record::Invocation(record))?;
                summary.invocations += 1;
            }
            if let Some(checkpoint) = checkpoint_of(store, &event.kind)?
                && seen_checkpoints.insert(checkpoint.id, ()).is_none()
            {
                written += write_record(&mut out, redactor, &Record::Checkpoint(checkpoint))?;
                summary.checkpoints += 1;
            }
            let artifact = match &event.kind {
                EventKind::ArtifactCreated { artifact } => Some(
                    store
                        .artifact(artifact.id)?
                        .unwrap_or_else(|| artifact.clone()),
                ),
                EventKind::ArtifactDeduplicated { artifact, .. } => store.artifact(*artifact)?,
                _ => None,
            };
            if let Some(mut artifact) = artifact
                && !seen_artifacts.contains_key(&artifact.id)
            {
                let payload = if artifact.evicted_at.is_none() {
                    store
                        .read_artifact_bytes(artifact.id)?
                        .map(|bytes| redacted_payload(redactor, bytes))
                } else {
                    None
                };
                artifact.size_bytes = payload.as_ref().map_or(0, |bytes| bytes.len() as u64);
                artifact.digest = payload
                    .as_ref()
                    .map(|bytes| hex::encode(sha2::Sha256::digest(bytes)));
                seen_artifacts.insert(artifact.id, artifact.clone());
                written += write_record(&mut out, redactor, &Record::Artifact(artifact.clone()))?;
                summary.artifacts += 1;
                if let Some(bytes) = payload {
                    written += write_record(
                        &mut out,
                        redactor,
                        &Record::ArtifactBytes {
                            id: artifact.id,
                            base64: BASE64.encode(bytes),
                        },
                    )?;
                    summary.artifact_payloads += 1;
                }
            }
            match &mut event.kind {
                EventKind::ArtifactCreated { artifact } => {
                    if let Some(recorded) = seen_artifacts.get(&artifact.id) {
                        *artifact = recorded.clone();
                    }
                }
                EventKind::ArtifactDeduplicated {
                    artifact, digest, ..
                } => {
                    // Legacy archives can have a dedup event without its record or payload.
                    *digest = seen_artifacts
                        .get(artifact)
                        .and_then(|recorded| recorded.digest.clone())
                        .unwrap_or_default();
                }
                _ => {}
            }
            written += write_record(&mut out, redactor, &Record::Event(event))?;
            summary.events += 1;
        }
        if count < 1_000 {
            break;
        }
    }
    out.flush().map_err(StoreError::Io)?;
    // A file that does not match its own manifest cannot be imported, so it
    // fails here rather than on whoever is handed it. The pin above makes
    // this unreachable for a concurrent append; an event *removed* from the
    // log mid-export would still reach it.
    if summary.events != events_in_run {
        return Err(invalid(format!(
            "run {run} changed while it was exported: the manifest promised {events_in_run} \
             events and {} were written",
            summary.events
        )));
    }
    drop(out);
    std::fs::rename(&partial, path).map_err(StoreError::Io)?;
    summary.bytes_written = written;
    Ok(summary)
}

/// Where the export is assembled before it is named. Beside the target, so
/// the rename onto it is on one filesystem and therefore atomic.
fn partial_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// How many events the run's log holds and the sequence the last of them
/// sits at, for the manifest and for the pin the stream stops at.
fn count_events(store: &dyn Store, run: RunId) -> Result<(u64, u64)> {
    let filter = EventFilter::for_run(run);
    let (mut after, mut total) = (0u64, 0u64);
    loop {
        let page = store.events(after, 1_000, &filter)?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        total += page.len() as u64;
        if page.len() < 1_000 {
            break;
        }
    }
    Ok((total, after))
}

/// Snapshots an event points at.
fn snapshot_ids(kind: &EventKind) -> Vec<aether_core::id::SnapshotId> {
    match kind {
        EventKind::SnapshotCreated { snapshot, .. }
        | EventKind::InvocationCompleted { snapshot, .. }
        | EventKind::InvocationFailed { snapshot, .. } => vec![*snapshot],
        _ => Vec::new(),
    }
}

/// The checkpoint an event carries or names.
fn checkpoint_of(store: &dyn Store, kind: &EventKind) -> Result<Option<Checkpoint>> {
    match kind {
        EventKind::CheckpointCreated { checkpoint } => Ok(Some(checkpoint.clone())),
        EventKind::CompactionCompleted { checkpoint, .. } => Ok(store.checkpoint(*checkpoint)?),
        _ => Ok(None),
    }
}

/// Every record leaves the machine through here, with anything
/// secret-shaped in it replaced.
///
/// Tool *output* is redacted before it enters the log; a tool *argument*
/// cannot be, because an approval answered later re-runs the call from that
/// very event - so the log keeps what the tool needs and the export, which
/// is where the content crosses a boundary, redacts instead. The argument is
/// not the only copy either: the same text is in the model's message, in the
/// invocation and in the snapshot assembled from them, which is why every
/// record goes through this rather than tool calls alone.
///
/// An artifact payload is redacted too, by [`redacted_payload`] before it is
/// encoded - running text patterns over base64 would corrupt the payload
/// rather than clean it, so it is the one record cleaned before it reaches
/// here rather than in here.
fn write_record(
    out: &mut BufWriter<File>,
    redactor: &aether_tools::Redactor,
    record: &Record,
) -> Result<u64> {
    if let Record::ArtifactBytes { id, .. } = record {
        // The payload was masked as bytes; only its framing is checked here.
        let framing = Record::ArtifactBytes {
            id: *id,
            base64: String::new(),
        };
        refuse_if_masked(
            redactor,
            serde_json::to_value(&framing).map_err(StoreError::Serde)?,
        )?;
        return write_line(out, record);
    }
    let mut value = serde_json::to_value(record).map_err(StoreError::Serde)?;
    aether_tools::redact_json(redactor, &mut value);
    if let Record::Artifact(artifact) = record
        && let Some(digest) = &artifact.digest
    {
        // Hashed from the masked payload, so it holds no secret; a known
        // secret that happens to be hex must not break the import's check.
        debug_assert!(value.get("digest").is_some(), "a set digest is serialized");
        value["digest"] = serde_json::Value::String(digest.clone());
    }
    // A known secret that matches a field name, tag or id would leave a record
    // the import cannot read, or one it reads without an optional field.
    let parsed =
        <Record as serde::Deserialize>::deserialize(&value).map_err(|_| format_collision())?;
    if serde_json::to_value(&parsed).map_err(StoreError::Serde)? != value {
        return Err(format_collision());
    }
    write_line(out, &value)
}

/// Refuse a line written as it is (the manifest, a payload's framing) if
/// masking would change it: a known secret there names part of the format.
fn refuse_if_masked(redactor: &aether_tools::Redactor, line: serde_json::Value) -> Result<()> {
    debug_assert!(line.is_object(), "every export line is a JSON object");
    let mut masked = line.clone();
    aether_tools::redact_json(redactor, &mut masked);
    if masked != line {
        return Err(format_collision());
    }
    Ok(())
}

/// Writing the masked line would break the import; writing it raw would carry
/// the secret. Refusing is the only safe answer.
fn format_collision() -> RuntimeError {
    invalid(
        "a known secret value matches part of the export format (a field name, tag or id), \
         so a masked record would not import as it was; export refused",
    )
}

/// An artifact's bytes as they leave the machine. A tool's artifact was
/// masked when it was stored, but a published one is kept as sent and an
/// older one may predate that, so the boundary masks again. The text runs of
/// a payload that is not all UTF-8 are masked too; its other bytes travel as
/// they are.
fn redacted_payload(redactor: &aether_tools::Redactor, bytes: Vec<u8>) -> Vec<u8> {
    match redactor.redact_bytes(&bytes) {
        std::borrow::Cow::Borrowed(_) => bytes,
        std::borrow::Cow::Owned(clean) => clean,
    }
}

fn write_line<T: Serialize>(out: &mut BufWriter<File>, value: &T) -> Result<u64> {
    let mut text = serde_json::to_string(value).map_err(StoreError::Serde)?;
    text.push('\n');
    out.write_all(text.as_bytes()).map_err(StoreError::Io)?;
    Ok(text.len() as u64)
}
