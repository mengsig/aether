//! How past use is split by what paid for it.

use aether_core::model::{ModelRef, Usage};
use aether_model::Models;

use super::by_billing;

fn used(tokens: u64, cost: f64) -> Usage {
    Usage {
        input_tokens: tokens,
        cost_usd: cost,
        ..Usage::default()
    }
}

#[test]
fn use_on_a_provider_no_longer_configured_is_unknown_never_an_api_charge() {
    let config = tempfile::tempdir().expect("config dir");
    let models = Models::load(Some(config.path().to_path_buf())).expect("models");
    let engine = ModelRef::new("claude-code", "haiku");
    let api = ModelRef::new("anthropic", "claude-haiku-5-5");
    let removed = ModelRef::new("retired-engine", "tutor");
    assert!(
        models
            .registry()
            .provider(&engine.provider)
            .is_some_and(|p| p.protocol.is_engine())
    );
    assert!(
        models
            .registry()
            .provider(&api.provider)
            .is_some_and(|p| !p.protocol.is_engine())
    );
    assert!(models.registry().provider(&removed.provider).is_none());

    let (on_engine, on_api, on_removed) = (used(10, 0.01), used(20, 0.02), used(40, 0.04));
    let split = by_billing(
        &models,
        [
            (&engine, &on_engine),
            (&api, &on_api),
            (&removed, &on_removed),
        ],
    );

    assert_eq!(split.subscription, on_engine);
    assert_eq!(
        split.api, on_api,
        "a removed provider's use is not billed to the API"
    );
    assert_eq!(split.unknown, on_removed);
}
