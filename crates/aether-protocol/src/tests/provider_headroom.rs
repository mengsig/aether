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
