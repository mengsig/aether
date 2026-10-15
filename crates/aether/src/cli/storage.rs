use super::connect;

pub async fn storage(
    endpoint: impl Into<aether_client::Endpoint>,
    gc: bool,
    dry_run: bool,
    json: bool,
) -> anyhow::Result<()> {
    let client = connect(&endpoint.into()).await?;
    let report = client.storage(gc, dry_run).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("Storage {}", if dry_run { "preview" } else { "cleanup" });
    for kind in [
        "workspaces",
        "backups",
        "logs",
        "engine_sessions",
        "artifacts",
        "event_history",
        "agent_temp",
    ] {
        let value = &report[kind];
        if let Some(bytes) = value["bytes"].as_u64() {
            println!("{kind}: {bytes} bytes");
        } else if let Some(kept) = value["bytes_kept"].as_u64() {
            let eligible = value["bytes_eligible"].as_u64().unwrap_or(0);
            println!("{kind}: {kept} bytes kept, {eligible} bytes eligible");
        } else {
            println!(
                "{kind}: {}",
                value["reason"]
                    .as_str()
                    .or(value["policy"].as_str())
                    .unwrap_or("see JSON report")
            );
        }
        if let Some(bytes) = value["bytes_removed"].as_u64() {
            println!("  {bytes} bytes freed");
        }
        if let Some(dirs) = value["dirs"].as_u64() {
            println!("  {dirs} agent temp dirs");
        }
        if let Some(unreadable) = value["unreadable"].as_u64().filter(|n| *n > 0) {
            println!("  {unreadable} directories could not be read");
        }
        if value["truncated"] == true {
            println!("  partial inventory: scan limit reached");
        }
        for key in ["entries", "workspaces"] {
            if let Some(entries) = value[key].as_array() {
                for entry in entries {
                    println!(
                        "  {} {}: {}",
                        entry["action"].as_str().unwrap_or("kept"),
                        entry["path"]
                            .as_str()
                            .or(entry["workspace"].as_str())
                            .unwrap_or("unknown path"),
                        entry["reason"].as_str().unwrap_or("see JSON report")
                    );
                }
            }
        }
    }
    if let Some(bytes) = report["available_bytes"].as_u64() {
        println!("Available: {bytes} bytes");
    }
    Ok(())
}
