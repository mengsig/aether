//! Secondary routes: Antigravity also serves other vendors' models, as
//! `antigravity/claude-opus-5-5` or `antigravity/gpt-oss-120b`. The vendor's
//! own provider is preferred whenever it is usable; the route through
//! Antigravity is for a machine that has nothing else.

use aether_core::model::ModelRef;

use crate::registry::Registry;

/// The provider that serves other vendors' models second-hand.
pub const SECONDARY_PROVIDER: &str = "antigravity";

/// The providers that serve `model` first-hand, most preferred first.
fn originals(model: &str) -> &'static [&'static str] {
    if model.starts_with("claude-") {
        &["claude-code", "anthropic"]
    } else if model.starts_with("gpt-") {
        &["codex", "openai"]
    } else {
        &[]
    }
}

/// Every provider that serves some secondary model first-hand.
pub const ORIGINAL_PROVIDERS: [&str; 4] = ["claude-code", "anthropic", "codex", "openai"];

/// Whether `model` is a secondary route that some original provider could serve.
pub fn has_original(model: &ModelRef) -> bool {
    model.provider == SECONDARY_PROVIDER && !originals(&model.model).is_empty()
}

/// The first-hand route for a secondary `model`: the first original provider
/// that `usable` accepts and that lists the same model id. `None` keeps `model`.
pub fn original_route(
    registry: &Registry,
    model: &ModelRef,
    usable: impl Fn(&str) -> bool,
) -> Option<ModelRef> {
    if model.provider != SECONDARY_PROVIDER {
        return None;
    }
    let original = originals(&model.model).iter().find(|provider| {
        let lists = registry
            .provider(provider)
            .is_some_and(|def| def.enabled && def.model(&model.model).is_some());
        lists && usable(provider)
    })?;
    debug_assert_ne!(
        *original, SECONDARY_PROVIDER,
        "a route never maps to itself"
    );
    Some(ModelRef::new(*original, model.model.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(model: &str, usable: &[&str]) -> Option<String> {
        let registry = Registry::builtin();
        let r = ModelRef::new(SECONDARY_PROVIDER, model);
        original_route(&registry, &r, |p| usable.contains(&p)).map(|r| r.to_string())
    }

    #[test]
    fn the_original_provider_wins_when_it_is_usable() {
        assert_eq!(
            route("claude-opus-5-5", &["claude-code", "codex"]).as_deref(),
            Some("claude-code/claude-opus-5-5")
        );
        assert_eq!(
            route("claude-sonnet-5-5", &["anthropic"]).as_deref(),
            Some("anthropic/claude-sonnet-5-5")
        );
    }

    #[test]
    fn the_secondary_route_stays_when_no_original_is_usable() {
        assert_eq!(route("claude-opus-5-5", &[]), None);
        assert_eq!(
            route("claude-opus-5-5", &["codex"]),
            None,
            "codex serves no Claude model"
        );
        // No first-hand provider lists GPT-OSS, so Antigravity keeps it.
        assert_eq!(route("gpt-oss-120b", &["codex", "openai"]), None);
        assert_eq!(route("gemini-4-neon", &["claude-code", "codex"]), None);
        let direct = ModelRef::new("claude-code", "claude-opus-5-5");
        assert!(original_route(&Registry::builtin(), &direct, |_| true).is_none());
    }

    #[test]
    fn every_original_is_listed_once() {
        for model in ["claude-opus-5-5", "gpt-6.2-sol"] {
            for provider in originals(model) {
                assert!(ORIGINAL_PROVIDERS.contains(provider), "{provider}");
            }
        }
        assert!(has_original(&ModelRef::new(
            SECONDARY_PROVIDER,
            "claude-opus-5-5"
        )));
        assert!(!has_original(&ModelRef::new(
            SECONDARY_PROVIDER,
            "gemini-4-neon"
        )));
        assert!(!has_original(&ModelRef::new(
            "claude-code",
            "claude-opus-5-5"
        )));
    }

    #[test]
    fn secondary_routes_resolve_through_antigravity() {
        let registry = Registry::builtin();
        let agy = registry.provider(SECONDARY_PROVIDER).expect("builtin");
        for id in ["claude-opus-5-5", "claude-sonnet-5-5", "gpt-oss-120b"] {
            let model = agy.model(id).unwrap_or_else(|| panic!("{id} is listed"));
            assert!(model.tier.is_none(), "{id} is never a tiered pick");
        }
    }
}
