//! Portable diagnostics contain structural data, never arbitrary user text.
use std::collections::BTreeSet;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_client::{Client, ClientError};
use aether_core::event::Event;
use aether_core::id::RunId;
use aether_protocol::EventQuery;
use anyhow::Context as _;
use serde_json::{Value, json};

const EVENT_LIMIT: usize = 200;
const LOG_BYTES: u64 = 64 * 1024;
const REDACTED: &str = "[REDACTED]";

pub async fn diagnose(
    socket: PathBuf,
    explicit_socket: bool,
    out: PathBuf,
    run: Option<super::RunRef>,
    last: usize,
    json: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!((1..=100).contains(&last), "--last must be 1..=100");
    // Before the bundle exists, so a reference that names nothing creates nothing.
    let run = match run {
        Some(run) => Some(requested_run(&socket, &run).await?),
        None => None,
    };
    let out = std::path::absolute(out)?;
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(&out)
        .with_context(|| format!("create new bundle {}", out.display()))?;
    // Refuse existing destinations rather than risking overwriting unrelated files.
    // A failed write leaves an incomplete directory without a complete manifest.
    let mut files = Vec::new();
    write_json(
        &out,
        &mut files,
        "build.json",
        &json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "debug_assertions": cfg!(debug_assertions),
            "protocol_version": aether_protocol::PROTOCOL_VERSION,
        }),
    )?;

    let connected = tokio::time::timeout(Duration::from_secs(5), Client::connect(&socket)).await;
    let (client, error) = match connected {
        Ok(Ok(client)) => (
            Some(client.with_request_timeout(Duration::from_secs(5))),
            None,
        ),
        Ok(Err(error)) => (None, Some(client_error(&error))),
        Err(_) => (None, Some(json!({"kind": "connection_timeout"}))),
    };
    let status = match &client {
        Some(client) => match client.status().await {
            Ok(status) => {
                let mut value = serde_json::to_value(status)?;
                redact_strings(&mut value);
                json!({"status": value})
            }
            Err(error) => json!({"error": client_error(&error)}),
        },
        None => json!({"error": error}),
    };
    write_json(
        &out,
        &mut files,
        "daemon.json",
        &json!({
            "socket": socket.display().to_string(), "connected": client.is_some(), "result": status,
        }),
    )?;

    let doctor = tokio::time::timeout(
        Duration::from_secs(30),
        super::diagnostics::doctor_report(socket, explicit_socket),
    )
    .await;
    let doctor = match doctor {
        Ok(report) => json!({
            "level": report.level,
            "checks": report.checks.iter().map(|check| json!({
                "name": check.name, "level": check.level,
                "detail": REDACTED, "fix": check.fix.as_ref().map(|_| REDACTED),
            })).collect::<Vec<_>>(),
        }),
        Err(_) => json!({"error": {"kind": "doctor_timeout"}}),
    };
    write_json(&out, &mut files, "doctor.json", &doctor)?;

    let config = match aether_model::config::load_config(&aether_model::config::config_dir(None)) {
        Ok(config) => {
            let mut value = serde_json::to_value(config)?;
            redact_strings(&mut value);
            json!({"effective": value})
        }
        Err(_) => json!({"error": "configuration could not be loaded; see doctor checks"}),
    };
    let mut registry = aether_model::Registry::builtin();
    let providers = aether_model::registry::load_providers_file(
        &aether_model::config::config_dir(None).join("providers.toml"),
    )
    .and_then(|file| file.map_or(Ok(()), |file| registry.apply_overrides(file)));
    let providers = match providers {
        Ok(()) => {
            let mut value = serde_json::to_value(registry.providers())?;
            redact_strings(&mut value);
            json!({"effective": value})
        }
        Err(_) => json!({"error": "provider configuration could not be loaded; see doctor checks"}),
    };
    let config = json!({"config": config, "providers": providers});
    write_json(&out, &mut files, "config.json", &config)?;
    let logs = match log_tail(&crate::logging::log_file("aetherd.log")) {
        Ok(logs) => logs,
        Err(error) => {
            json!({"error": {"kind": format!("{:?}", error.kind()), "os_code": error.raw_os_error()}})
        }
    };
    write_json(&out, &mut files, "logs.json", &logs)?;

    let history = match &client {
        Some(client) => history(client, run, last).await,
        None => json!({"error": "daemon unavailable", "requested_run": run, "last": last}),
    };
    write_json(&out, &mut files, "history.json", &history)?;
    let manifest = json!({
        "format_version": 1,
        "created_at": aether_core::time::now().to_rfc3339(),
        "files": files,
        "redaction": "All free-form strings, including config strings/map keys, provider headers, doctor details, status strings and log messages are omitted. Event payloads, credentials and transcripts are never included. Socket path is retained.",
        "history": {"event_limit": EVENT_LIMIT, "run": run, "last": last,
            "replay_cursors": "Derived from the bounded event window; frames may be partial."},
        "logs": {"max_bytes": LOG_BYTES, "max_lines": 200, "current_segment_only": true},
    });
    write_json(&out, &mut files, "manifest.json", &manifest)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": out.display().to_string(), "manifest": manifest,
            }))?
        );
    } else {
        println!(
            "bundle written to {}",
            super::sane(&out.display().to_string())
        );
        println!("manifest: {}", files.join(", "));
    }
    Ok(())
}

fn write_json(
    out: &Path,
    files: &mut Vec<&'static str>,
    name: &'static str,
    value: &Value,
) -> anyhow::Result<()> {
    let mut file = aether_core::private_file::create_new(&out.join(name))
        .with_context(|| format!("create bundle file {name}"))?;
    file.write_all(&serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write bundle file {name}"))?;
    files.push(name);
    Ok(())
}

// Error classes and OS codes diagnose failures without copying a server's error text.
/// A full id is read as it is, even with no daemon: this command must work
/// without one. Anything else asks the daemon which run it names.
async fn requested_run(socket: &Path, run: &super::RunRef) -> anyhow::Result<RunId> {
    if let Some(id) = run.full_id() {
        return Ok(id);
    }
    let connected = tokio::time::timeout(Duration::from_secs(5), Client::connect(socket)).await;
    let client = connected
        .context("timed out reaching the daemon to resolve --run")?
        .context("reaching the daemon to resolve --run; give the full run id")?;
    let client = client.with_request_timeout(Duration::from_secs(5));
    run.resolve(&client, &socket.into()).await
}

fn client_error(error: &ClientError) -> Value {
    let kind = match error {
        ClientError::Io(error) => {
            return json!({"kind": format!("{:?}", error.kind()), "os_code": error.raw_os_error()});
        }
        ClientError::Protocol(_) => "protocol",
        ClientError::Codec(_) => "codec",
        ClientError::Disconnected => "disconnected",
        ClientError::Incompatible(_) => "incompatible",
        ClientError::Unauthorized(_) => "unauthorized",
        ClientError::Unexpected { .. } => "unexpected_reply",
        ClientError::Malformed(_) => "malformed",
        ClientError::InvalidUsage(_) => "invalid_usage",
        ClientError::TimedOut { .. } => "request_timeout",
        ClientError::DaemonStartup(_) => "daemon_startup",
        ClientError::Ssh(_) => "ssh_failure",
        ClientError::DeliveryUnconfirmed(_) => "delivery_unconfirmed",
    };
    json!({"kind": kind, "detail": REDACTED})
}

fn redact_strings(value: &mut Value) {
    match value {
        Value::String(text) => *text = REDACTED.into(),
        Value::Array(values) => values.iter_mut().for_each(redact_strings),
        Value::Object(values) => {
            for (key, value) in values {
                // These are the configuration's arbitrary-key maps. Keys can be
                // credentials as well, so omit the whole map before recursion.
                if matches!(
                    key.as_str(),
                    "env"
                        | "classes"
                        | "headers"
                        | "extra_authorize_params"
                        | "extra_token_params"
                        | "extensions"
                ) {
                    *value = json!(REDACTED);
                } else {
                    redact_strings(value);
                }
            }
        }
        _ => {}
    }
}

fn log_tail(path: &Path) -> std::io::Result<Value> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let offset = metadata.len().saturating_sub(LOG_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(LOG_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    // Discard a potentially partial first line after seeking into the file.
    let text = if offset > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    let lines = text
        .lines()
        .rev()
        .take(200)
        .map(|line| {
            let mut words = line.split_whitespace();
            let at = words
                .next()
                .and_then(|at| at.parse::<aether_core::time::Timestamp>().ok());
            let level = words
                .next()
                .filter(|word| matches!(*word, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR"));
            json!({"at": at, "level": level, "message": REDACTED})
        })
        .collect::<Vec<_>>();
    Ok(json!({"lines_newest_first": lines, "byte_window_truncated": offset > 0}))
}

fn event_metadata(event: &Event) -> Value {
    json!({"id": event.id, "seq": event.seq, "at": event.at,
        "agent": event.agent_id, "run": event.run, "task": event.task_id,
        "type": event.kind.type_name()})
}

async fn recent(client: &Client, run: Option<RunId>) -> Result<Vec<Event>, ClientError> {
    client
        .events(
            0,
            EVENT_LIMIT,
            EventQuery {
                before: Some(u64::MAX),
                run,
                ..Default::default()
            },
        )
        .await
}

async fn history(client: &Client, run: Option<RunId>, last: usize) -> Value {
    let events = match recent(client, None).await {
        Ok(events) => {
            json!({"events_newest_first": events.iter().map(event_metadata).collect::<Vec<_>>() })
        }
        Err(error) => json!({"error": client_error(&error)}),
    };
    let runs = match run {
        Some(run) => vec![run],
        None => match client.runs(last).await {
            Ok(runs) => runs.into_iter().map(|run| run.run).collect(),
            Err(error) => return json!({"recent": events, "runs_error": client_error(&error)}),
        },
    };
    let mut summaries = Vec::new();
    for run in runs {
        match recent(client, Some(run)).await {
            Ok(mut events) => {
                events.reverse();
                let agents = events
                    .iter()
                    .filter_map(|event| event.agent_id)
                    .collect::<BTreeSet<_>>();
                let cursors = agents
                    .into_iter()
                    .flat_map(|agent| aether_core::replay::build(agent, &events))
                    .map(|frame| {
                        json!({"agent": frame.agent, "turn": frame.turn,
                        "from_seq": frame.from_seq, "to_seq": frame.to_seq,
                        "outcome": frame.outcome.as_str(), "started_at": frame.started_at})
                    })
                    .collect::<Vec<_>>();
                summaries.push(json!({"run": run, "window_may_be_truncated": events.len() == EVENT_LIMIT,
                    "after": events.last().map(|event| event.seq), "before": events.first().map(|event| event.seq),
                    "events": events.iter().map(event_metadata).collect::<Vec<_>>(), "replay_cursors": cursors}));
            }
            Err(error) => summaries.push(json!({"run": run, "error": client_error(&error)})),
        }
    }
    json!({"recent": events, "runs": summaries})
}
