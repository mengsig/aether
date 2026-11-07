#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use aether_core::view_modes::{Catalog, Category, Density, Diagnostic, Scope};
use aether_protocol::{Access, Command, Introduced, Reply, ViewMode, ViewModes, features};
use serde_json::json;

#[test]
fn list_view_modes_is_a_feature_gated_read_without_arguments() {
    let command: Command = serde_json::from_value(json!({"cmd": "list_view_modes"})).unwrap();
    assert_eq!(command, Command::ListViewModes);
    assert_eq!(command.access(), Access::Observe);
    assert_eq!(
        command.introduced(),
        Introduced::Feature(features::VIEW_MODES)
    );
    assert_eq!(
        serde_json::to_value(command).unwrap(),
        json!({"cmd": "list_view_modes"})
    );
}

#[test]
fn view_modes_wire_shape_preserves_nullable_fields() {
    let reply = Reply::ViewModes(ViewModes {
        modes: vec![ViewMode {
            name: "calm".into(),
            label: None,
            description: None,
            density: Density::Comfortable,
            show: vec![Category::UserText],
            scope: Some(Scope {
                name: None,
                role: Some("archon-orchestrator".into()),
            }),
            builtin: false,
        }],
        diagnostics: vec![Diagnostic {
            source: "bad.toml".into(),
            reason: "invalid".into(),
        }],
    });
    let expected = json!({"reply": "view_modes", "data": {
        "modes": [{"name": "calm", "label": null, "description": null, "density": "comfortable",
            "show": ["user_text"], "scope": {"name": null, "role": "archon-orchestrator"}, "builtin": false}],
        "diagnostics": [{"source": "bad.toml", "reason": "invalid"}]
    }});
    assert_eq!(serde_json::to_value(&reply).unwrap(), expected);
    assert_eq!(serde_json::from_value::<Reply>(expected).unwrap(), reply);
    let standard = ViewMode::from(&Catalog::default().modes["standard"]);
    assert_eq!(standard.show, Category::ALL);
    assert_eq!(
        serde_json::to_value(standard).unwrap()["scope"],
        serde_json::Value::Null
    );
}
