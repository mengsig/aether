use super::*;
use aether_core::view_modes::{Category, Density, Scope};
use aether_protocol::ViewModes;

const CALM: &str = r#"version = 1
label = "Calm"
description = "Coordination"
density = "compact"
show = ["user_text", "assistant_text"]
[scope]
role = "archon-orchestrator"
"#;

async fn catalog(daemon: &Daemon) -> ViewModes {
    let command: Command =
        serde_json::from_value(serde_json::json!({"cmd": "list_view_modes"})).unwrap();
    assert_eq!(command.access(), aether_protocol::Access::Observe);
    let Reply::ViewModes(catalog) = dispatch(daemon, command, Actor::user()).await.unwrap() else {
        panic!("view modes reply");
    };
    assert!(catalog.modes.len() >= 2);
    catalog
}

#[tokio::test]
async fn list_view_modes_reads_scoped_modes_and_reports_bare_filenames() {
    let (daemon, config) = daemon().await;
    let modes = config.path().join("modes");
    std::fs::create_dir(&modes).unwrap();
    for (name, text) in [
        ("calm.toml", CALM),
        ("bad.toml", "version = ["),
        ("q.toml", CALM),
    ] {
        std::fs::write(modes.join(name), text).unwrap();
    }
    let catalog = catalog(&daemon).await;
    assert_eq!(
        catalog
            .modes
            .iter()
            .map(|mode| mode.name.as_str())
            .collect::<Vec<_>>(),
        ["standard", "focus", "calm"]
    );
    assert_eq!(catalog.modes[0].show, Category::ALL);
    assert_eq!(catalog.modes[0].density, Density::Comfortable);
    assert!(catalog.modes[0].builtin && catalog.modes[1].builtin);
    assert_calm(&catalog.modes[2]);
    assert_eq!(
        catalog
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.source.as_str())
            .collect::<Vec<_>>(),
        ["bad.toml", "q.toml"]
    );
    assert!(catalog.diagnostics[0].reason.contains("TOML"));
    assert!(catalog.diagnostics[1].reason.contains("reserved"));
    let wire = serde_json::to_value(Reply::ViewModes(catalog)).unwrap();
    assert_eq!(wire["reply"], "view_modes");
    assert_eq!(
        wire["data"]["modes"][2]["scope"]["name"],
        serde_json::Value::Null
    );
    assert!(
        !wire
            .to_string()
            .contains(&config.path().display().to_string())
    );
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn each_list_view_modes_call_rediscovers_files_in_name_order() {
    let (daemon, config) = daemon().await;
    assert_eq!(catalog(&daemon).await.modes.len(), 2);
    let modes = config.path().join("modes");
    std::fs::create_dir(&modes).unwrap();
    for name in ["zebra", "calm"] {
        std::fs::write(modes.join(format!("{name}.toml")), CALM).unwrap();
    }
    assert_eq!(
        catalog(&daemon)
            .await
            .modes
            .iter()
            .map(|mode| mode.name.as_str())
            .collect::<Vec<_>>(),
        ["standard", "focus", "calm", "zebra"]
    );
    std::fs::write(modes.join("calm.toml"), "invalid").unwrap();
    let catalog = catalog(&daemon).await;
    assert_eq!(catalog.modes.len(), 3);
    assert_eq!(catalog.diagnostics[0].source, "calm.toml");
    daemon.runtime.shutdown().await;
}

fn assert_calm(calm: &aether_protocol::ViewMode) {
    assert!(!calm.builtin);
    assert_eq!(calm.label.as_deref(), Some("Calm"));
    assert_eq!(calm.description.as_deref(), Some("Coordination"));
    assert_eq!(calm.density, Density::Compact);
    assert_eq!(calm.show, [Category::UserText, Category::AssistantText]);
    assert_eq!(
        calm.scope,
        Some(Scope {
            name: None,
            role: Some("archon-orchestrator".into())
        })
    );
}

#[tokio::test]
async fn list_view_modes_reports_directory_failures_without_host_paths() {
    let (daemon, config) = daemon().await;
    std::fs::write(config.path().join("modes"), "not a directory").unwrap();
    let catalog = catalog(&daemon).await;
    assert_eq!(catalog.modes.len(), 2);
    assert_eq!(catalog.diagnostics.len(), 1);
    assert_eq!(catalog.diagnostics[0].source, "modes");
    assert!(!catalog.diagnostics[0].reason.is_empty());
    assert!(
        !serde_json::to_string(&catalog)
            .unwrap()
            .contains(&config.path().display().to_string())
    );
    daemon.runtime.shutdown().await;
}
