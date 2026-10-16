use super::connect;
use super::units;
use serde_json::Value;

pub async fn storage(
    endpoint: impl Into<aether_client::Endpoint>,
    gc: bool,
    dry_run: bool,
    prune_build_output: bool,
    json: bool,
) -> anyhow::Result<()> {
    let client = connect(&endpoint.into()).await?;
    let report = client.storage(gc, dry_run, prune_build_output).await?;
    println!("{}", storage_text(&report, dry_run, json)?);
    Ok(())
}

/// Exact bytes for `--json`, scaled ones for a person.
fn storage_text(report: &Value, dry_run: bool, json: bool) -> anyhow::Result<String> {
    if json {
        return Ok(serde_json::to_string_pretty(report)?);
    }
    Ok(human_storage(report, dry_run).join("\n"))
}

const KINDS: [&str; 9] = [
    "workspaces",
    "backups",
    "logs",
    "engine_sessions",
    "artifacts",
    "event_history",
    "agent_temp",
    "build_cache",
    "bridge_copies",
];

/// The report a person reads, sizes in binary units; `--json` keeps bytes.
fn human_storage(report: &Value, dry_run: bool) -> Vec<String> {
    let mut out = vec![format!(
        "Storage {}",
        if dry_run { "preview" } else { "cleanup" }
    )];
    for kind in KINDS {
        out.extend(kind_lines(kind, &report[kind]));
    }
    if let Some(bytes) = report["available_bytes"].as_u64() {
        out.push(format!("Available: {}", units::bytes(bytes)));
    }
    debug_assert!(out.len() > KINDS.len(), "every kind says at least one line");
    out
}

fn kind_lines(kind: &str, value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(bytes) = value["bytes"].as_u64() {
        out.push(format!("{kind}: {}", units::bytes(bytes)));
    } else if let Some(kept) = value["bytes_kept"].as_u64() {
        let eligible = value["bytes_eligible"].as_u64().unwrap_or(0);
        out.push(format!(
            "{kind}: {} kept, {} eligible",
            units::bytes(kept),
            units::bytes(eligible)
        ));
    } else {
        out.push(format!(
            "{kind}: {}",
            value["reason"]
                .as_str()
                .or(value["policy"].as_str())
                .unwrap_or("see JSON report")
        ));
    }
    if let Some(bytes) = value["bytes_removed"].as_u64() {
        out.push(format!("  {} freed", units::bytes(bytes)));
    }
    if let Some(dirs) = value["dirs"].as_u64() {
        out.push(format!("  {dirs} agent temp dirs"));
    }
    if let Some(partitions) = value["partitions"].as_u64() {
        out.push(format!("  {partitions} build cache partitions"));
    }
    if let Some(unreadable) = value["unreadable"].as_u64().filter(|n| *n > 0) {
        out.push(format!("  {unreadable} directories could not be read"));
    }
    if value["truncated"] == true {
        out.push("  partial inventory: scan limit reached".to_owned());
    }
    for key in ["entries", "workspaces"] {
        for entry in value[key].as_array().into_iter().flatten() {
            out.push(format!(
                "  {} {}: {}",
                entry["action"].as_str().unwrap_or("kept"),
                entry["path"]
                    .as_str()
                    .or(entry["workspace"].as_str())
                    .unwrap_or("unknown path"),
                entry["reason"].as_str().unwrap_or("see JSON report")
            ));
            out.extend(build_output_lines(entry));
        }
    }
    debug_assert!(
        out[0].starts_with(kind),
        "a kind's lines open with its name"
    );
    out
}

fn build_output_lines(entry: &Value) -> impl Iterator<Item = String> + '_ {
    entry["build_output"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|output| {
            format!(
                "    build output {} {}: {}",
                output["action"].as_str().unwrap_or("kept"),
                output["path"].as_str().unwrap_or("unknown path"),
                units::bytes(output["bytes"].as_u64().unwrap_or(0))
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report() -> Value {
        json!({
            "workspaces": {
                "bytes_kept": 106_801_321_380_u64, "bytes_eligible": 2048,
                "bytes_removed": 1_048_576,
                "workspaces": [{"action": "kept", "workspace": "/w/a", "reason": "live",
                    "build_output": [{"action": "pruned", "path": "/w/a/target", "bytes": 5_368_709_120_u64}]}],
            },
            "logs": {"bytes": 812},
            "backups": {"policy": "kept forever"},
            "available_bytes": 1_099_511_627_776_u64,
        })
    }

    #[test]
    fn a_person_reads_sizes_in_binary_units() {
        let text = storage_text(&report(), true, false).expect("renders");
        let expected = [
            "Storage preview",
            "workspaces: 99.5 GiB kept, 2.00 KiB eligible",
            "  1.00 MiB freed",
            "  kept /w/a: live",
            "    build output pruned /w/a/target: 5.00 GiB",
            "backups: kept forever",
            "logs: 812 B",
        ];
        for line in expected {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in\n{text}"
            );
        }
        assert!(text.ends_with("Available: 1.00 TiB"), "{text}");
        assert!(!text.contains("106801321380"), "{text}");
    }

    #[test]
    fn json_is_the_report_unchanged() {
        let report = report();
        let text = storage_text(&report, true, true).expect("renders");
        assert_eq!(
            text,
            serde_json::to_string_pretty(&report).expect("serializes")
        );
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["workspaces"]["bytes_kept"], 106_801_321_380_u64);
        assert_eq!(value["available_bytes"], 1_099_511_627_776_u64);
    }
}
