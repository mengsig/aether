use super::*;
use aether_core::provider::{ProviderHeadroom, ProviderUsage};
use serde_json::json;

#[test]
fn providers_headroom_json_shape_is_additive() {
    let at: Timestamp = "2030-01-02T15:15:00Z".parse().unwrap();
    let reset: Timestamp = "2030-01-02T15:30:00Z".parse().unwrap();
    let usage = ProviderUsage::from_engine_payload(
        &json!({
            "primary":{"usedPercent":38,"windowDurationMins":300,"resetsAt":reset.timestamp()}
        }),
        at - chrono::Duration::seconds(30),
    )
    .unwrap();
    let mut provider = provider_info();
    provider.headroom =
        ProviderHeadroom::from_usage(Some(&usage), at, std::time::Duration::from_secs(900));
    let reply = Reply::Providers(vec![provider.clone()]);
    let wire = serde_json::to_value(&reply).unwrap();
    assert_eq!(wire["reply"], "providers");
    assert_eq!(
        wire["data"][0]["headroom"],
        json!({
            "windows": [{"name":"primary", "window_minutes":300, "remaining_percent":62,
                "resets_at":"2030-01-02T15:30:00Z", "resets_in_seconds":900}],
            "observed_at":"2030-01-02T15:14:30Z", "age_seconds":30, "stale":false
        })
    );
    assert_eq!(
        serde_json::from_value::<Reply>(wire.clone()).unwrap(),
        reply
    );
    // Older clients ignore the additive projection, including its nested fields.
    #[derive(Deserialize)]
    struct LegacyProvider {
        provider: String,
        health: ProviderHealth,
    }
    #[derive(Deserialize)]
    #[serde(tag = "reply", content = "data", rename_all = "snake_case")]
    enum LegacyReply {
        Providers(Vec<LegacyProvider>),
    }
    let LegacyReply::Providers(old) = serde_json::from_value(wire).unwrap();
    assert_eq!(old[0].provider, provider.provider);
    assert_eq!(old[0].health, provider.health);
    let mut old = serde_json::to_value(provider).unwrap();
    old.as_object_mut().unwrap().remove("headroom");
    let current: ProviderInfo = serde_json::from_value(old).unwrap();
    assert_eq!(current.headroom, ProviderHeadroom::default());
    assert!(current.headroom.stale);
}

#[test]
fn refresh_provider_usage_command_roundtrips_and_requires_operate_access() {
    for (wire, provider) in [
        (json!({"cmd":"refresh_provider_usage"}), None),
        (
            json!({"cmd":"refresh_provider_usage", "provider":null}),
            None,
        ),
        (
            json!({"cmd":"refresh_provider_usage", "provider":"codex"}),
            Some("codex".into()),
        ),
    ] {
        let command: Command = serde_json::from_value(wire).unwrap();
        assert_eq!(command, Command::RefreshProviderUsage { provider });
        assert_eq!(command.access(), Access::Operate);
        assert_eq!(
            command.introduced(),
            crate::Introduced::Feature(crate::features::PROVIDER_USAGE_REFRESH)
        );
        assert_eq!(command.name(), "refresh_provider_usage");
        assert_eq!(
            decode::<Command>(&encode(&command).unwrap()).unwrap(),
            command
        );
    }
    assert_eq!(Command::Providers.access(), Access::Observe);
    assert!(
        serde_json::from_value::<Command>(
            json!({"cmd":"refresh_provider_usage", "provider":false})
        )
        .is_err()
    );
}

#[test]
fn refresh_provider_usage_reply_preserves_status_and_per_provider_error() {
    let data = json!({"providers":[
        {"provider":"codex", "headroom":ProviderHeadroom::default(), "status":"refreshed"},
        {"provider":"custom-engine", "headroom":ProviderHeadroom::default(),
            "status":"error", "error":"probe timed out"}
    ]});
    let reply = Reply::ProviderUsage(data.clone());
    let encoded = serde_json::to_value(&reply).unwrap();
    assert_eq!(encoded["reply"], "provider_usage");
    assert_eq!(encoded["data"], data);
    assert_eq!(decode::<Reply>(&encode(&reply).unwrap()).unwrap(), reply);
}

#[test]
fn generated_schema_includes_refresh_command_and_usage_reply() {
    let schema = crate::schema::schema();
    let command = schema["$defs"]["ClientFrame"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["cmd"]["const"] == "refresh_provider_usage")
        .unwrap();
    assert!(command["properties"].get("provider").is_some());
    assert!(
        !command["required"]
            .as_array()
            .unwrap()
            .contains(&json!("provider"))
    );
    let reply = schema["$defs"]["Reply"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|variant| variant["properties"]["reply"]["const"] == "provider_usage")
        .unwrap();
    assert!(reply["properties"].get("data").is_some());
    assert!(
        reply["required"]
            .as_array()
            .unwrap()
            .contains(&json!("data"))
    );
}
