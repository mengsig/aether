use super::*;

/// `ALL` and `as_str` come from one macro list, so they cannot diverge.
/// What is left to check is that the list itself has no duplicate, and
/// that the serde name every provider file is written against is the same
/// string `as_str` reports.
/// The blocker fix made the Codex dialect ship inert. The config surface
/// must not be able to restore what it removed.
#[test]
fn a_subscription_login_is_refused_on_an_http_adapter() {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        r#"
[providers.openai-codex]
enabled = true
base_url = "https://chatgpt.com/backend-api/codex"
[providers.openai-codex.auth.oauth]
flow = "openai-codex"
"#,
    )
    .expect("toml");
    let err = reg
        .apply_overrides(file)
        .expect_err("a subscription over an HTTP adapter must be refused");
    let RegistryError::Config { reason, .. } = &err else {
        panic!("expected a config error, got {err:?}");
    };
    assert!(
        reason.contains("openai-codex") && reason.contains("subscription"),
        "{reason}"
    );

    // The same flow on the engine that owns it is fine, and an ordinary
    // non-subscription OAuth over HTTP is untouched.
    let mut reg = Registry::builtin();
    let ok: ProvidersFile = toml::from_str(
        r#"
[providers.openrouter]
enabled = true
[providers.openrouter.auth.oauth]
flow = "openrouter"
"#,
    )
    .expect("toml");
    reg.apply_overrides(ok)
        .expect("openrouter oauth is not a subscription");
}

/// The builtin registry with `codex/gpt-6-astra` re-registered by an
/// operator's `providers.toml` without a price; `openai/gpt-6-astra` keeps its.
fn registry_with_unpriced_codex_astra() -> Registry {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        r#"
[[providers.codex.add_models]]
id = "gpt-6-astra"
aliases = ["astra"]
[providers.codex.add_models.capabilities]
context_window = 272000
max_output_tokens = 128000
"#,
    )
    .expect("toml");
    reg.apply_overrides(file).expect("an unpriced engine model");
    reg
}

/// The estimate's whole foundation: a subscription model with no price of
/// its own matched to the API model the registry does price.
#[test]
fn an_unpriced_subscription_model_borrows_its_api_twins_price() {
    let reg = registry_with_unpriced_codex_astra();
    let resolve = |r: &str| reg.resolve(&ModelRef::parse(r).expect(r)).expect(r);

    let astra = resolve("codex/gpt-6-astra");
    assert_eq!(
        astra.model.capabilities.cost, None,
        "the operator's entry carries no price, which is why an estimate exists"
    );
    let twin = reg
        .api_equivalent(&astra)
        .expect("openai lists gpt-6-astra");
    assert_eq!(twin.id, "gpt-6-astra");
    let price = reg
        .api_equivalent_price(&astra)
        .expect("and it carries a list price");
    assert_eq!(price, twin.capabilities.cost.expect("twin price"));
    assert_eq!(price.input_per_mtok, 10.0);

    // An alias resolves to the same model, so it estimates the same way.
    assert_eq!(
        reg.api_equivalent_price(&resolve("codex/astra")),
        Some(price)
    );
}

/// The builtin registry plus an operator's `gpt-unpriced` on both `codex` and
/// `openai`, neither with a price: a twin that publishes no per-token rate.
fn registry_with_an_unpriced_twin() -> Registry {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        r#"
[[providers.codex.add_models]]
id = "gpt-unpriced"
[providers.codex.add_models.capabilities]
context_window = 272000
max_output_tokens = 128000
[[providers.openai.add_models]]
id = "gpt-unpriced"
[providers.openai.add_models.capabilities]
context_window = 272000
max_output_tokens = 128000
"#,
    )
    .expect("toml");
    reg.apply_overrides(file).expect("unpriced models");
    reg
}

#[test]
fn a_model_with_no_priced_twin_is_left_unestimated() {
    let reg = registry_with_an_unpriced_twin();
    let resolve = |r: &str| reg.resolve(&ModelRef::parse(r).expect(r)).expect(r);

    // `default` is whatever the CLI picks; no API table lists it, and an
    // open model list must not be allowed to invent a price for it.
    let fallback = resolve("codex/default");
    assert!(reg.api_equivalent(&fallback).is_none());
    assert_eq!(reg.api_equivalent_price(&fallback), None);

    // The twin exists but publishes no per-token price: still no estimate.
    let unpriced_twin = resolve("codex/gpt-unpriced");
    assert!(reg.api_equivalent(&unpriced_twin).is_some());
    assert_eq!(reg.api_equivalent_price(&unpriced_twin), None);

    // An API model is its own price; nothing to estimate from.
    let api = resolve("openai/gpt-6-astra");
    assert_eq!(reg.api_equivalent(&api), None);
    assert_eq!(reg.api_equivalent_price(&api), None);
}

/// `default` is the standard configuration, and the CLI only
/// says which model it picked once the session opens. That report is the
/// one chance to price the turn - within the engine's own model list.
#[test]
fn a_session_prices_the_default_model_the_engine_actually_picked() {
    let reg = registry_with_an_unpriced_twin();
    let resolve = |r: &str| reg.resolve(&ModelRef::parse(r).expect(r)).expect(r);

    // Claude Code publishes the subscription's list price, so the session
    // model is billed, not estimated - by the full API id it reports.
    let claude = resolve("claude-code/default");
    let opus = resolve("claude-code/opus");
    assert_eq!(opus.model.id, "claude-opus-5-5");
    assert_eq!(
        reg.session_model_price(&claude, "claude-opus-5-5"),
        Some(ModelPrice::Billed(opus.model.capabilities.cost.unwrap()))
    );
    let fable = reg
        .provider("claude-code")
        .expect("claude-code")
        .model("fable")
        .expect("fable")
        .capabilities
        .cost
        .expect("fable is priced");
    assert_eq!(
        reg.session_model_price(&claude, "claude-fable-5-1"),
        Some(ModelPrice::Billed(fable)),
        "the session named a model this engine prices"
    );

    // Claude Code reports a model by its full id; each bills as itself.
    for (reported, id) in [
        ("claude-sonnet-5-5", "sonnet"),
        ("claude-haiku-5-5", "haiku"),
    ] {
        let listed = resolve(&format!("claude-code/{id}"));
        assert_eq!(
            reg.session_model_price(&claude, reported),
            Some(ModelPrice::Billed(listed.model.capabilities.cost.unwrap())),
            "{reported}"
        );
    }

    // Codex is a subscription with no price of its own: the session model is
    // estimated from the OpenAI API list price, never billed.
    let codex = resolve("codex/default");
    let astra = resolve("openai/gpt-6-astra")
        .model
        .capabilities
        .cost
        .expect("openai prices gpt-6-astra");
    assert_eq!(
        reg.session_model_price(&codex, "gpt-6-astra"),
        Some(ModelPrice::ApiEquivalent(astra)),
        "the session named a model with an API twin"
    );
    assert_eq!(
        reg.session_model_price(&codex, "astra"),
        Some(ModelPrice::ApiEquivalent(astra)),
        "an alias is the same model, so it prices the same way"
    );

    // A model neither table lists gets no price: the turn stays unpriced
    // rather than borrowing a number from somewhere it does not belong.
    assert_eq!(reg.session_model_price(&codex, "gpt-9-nova"), None);
    assert_eq!(reg.session_model_price(&claude, ""), None);
    assert_eq!(
        reg.session_model_price(&codex, "default"),
        None,
        "the placeholder prices nothing, however it is reported"
    );
    // Listed by the engine, but no price on either table.
    assert_eq!(reg.session_model_price(&codex, "gpt-unpriced"), None);

    // A configured model that already has a price or an estimate is the
    // cost authority; what the session reports cannot move it.
    for configured in ["claude-code/sonnet", "codex/gpt-6-astra"] {
        let configured = resolve(configured);
        for reported in ["claude-fable-5-1", "gpt-6-astra", "haiku"] {
            assert_eq!(
                reg.session_model_price(&configured, reported),
                None,
                "{} is priced already; {reported} must not reprice it",
                configured.model.id
            );
        }
    }
}

/// A session model with no price of its own can still only be estimated,
/// from its API twin, never billed.
#[test]
fn a_session_model_without_a_price_is_estimated_from_its_twin() {
    let reg = registry_with_unpriced_codex_astra();
    let codex = reg
        .resolve(&ModelRef::new("codex", "default"))
        .expect("codex/default");
    let twin = reg
        .provider("openai")
        .and_then(|openai| openai.model("gpt-6-astra"))
        .and_then(|astra| astra.capabilities.cost)
        .expect("openai prices gpt-6-astra");
    assert_eq!(
        reg.session_model_price(&codex, "astra"),
        Some(ModelPrice::ApiEquivalent(twin))
    );
}

/// `claude-code/sonnet` already carries `anthropic/claude-sonnet-5-5`'s list
/// price, so its cost is priced rather than estimated - but the twin is
/// still found, which is what keeps the two tables honest.
#[test]
fn a_priced_subscription_model_is_never_estimated_over() {
    let reg = Registry::builtin();
    let sonnet = reg
        .resolve(&ModelRef::new("claude-code", "sonnet"))
        .expect("claude-code/sonnet");
    let twin = reg.api_equivalent(&sonnet).expect("anthropic twin");
    assert_eq!(twin.id, "claude-sonnet-5-5");
    assert_eq!(
        sonnet.model.capabilities.cost, twin.capabilities.cost,
        "the subscription model is priced like its twin"
    );
    assert_eq!(
        reg.api_equivalent_price(&sonnet),
        None,
        "a priced model needs no estimate"
    );
}

#[test]
fn every_protocol_is_listed_exactly_once_under_one_name() {
    let mut names: Vec<&str> = Protocol::ALL.iter().map(|p| p.as_str()).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "ALL must not repeat a protocol");
    for p in Protocol::ALL {
        let json = serde_json::to_string(p).expect("serialize");
        assert_eq!(
            json,
            format!("\"{}\"", p.as_str()),
            "the wire name and as_str disagree"
        );
    }
}

#[test]
fn a_zero_max_concurrent_clears_the_cap() {
    let mut def = ProviderDef {
        id: "p".into(),
        name: "p".into(),
        protocol: Protocol::AnthropicMessages,
        base_url: "http://127.0.0.1:1".into(),
        auth: AuthMethods::none(),
        headers: BTreeMap::new(),
        models: Vec::new(),
        default_capabilities: None,
        flags: ProviderFlags::default(),
        max_concurrent: Some(3),
        family: None,
        enabled: true,
        usage_probe: Default::default(),
    };
    apply_override(
        &mut def,
        ProviderOverride {
            max_concurrent: Some(0),
            ..Default::default()
        },
    )
    .expect("override applies");
    assert_eq!(def.max_concurrent, None, "0 means no cap, not a cap of 0");
    apply_override(
        &mut def,
        ProviderOverride {
            max_concurrent: Some(2),
            ..Default::default()
        },
    )
    .expect("override applies");
    assert_eq!(def.max_concurrent, Some(2));
}

#[test]
fn misplaced_model_table_is_reported_with_a_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("providers.toml");
    std::fs::write(&path, "[model]\ndefault = \"anthropic/claude-opus-5-5\"\n").expect("write");
    let err = load_providers_file(&path).expect_err("unknown table must fail");
    assert!(err.to_string().contains("config.toml"), "{err}");
}

#[test]
fn builtin_resolves_aliases_and_open_lists() {
    let reg = Registry::builtin();
    let opus = reg
        .resolve(&ModelRef::new("anthropic", "opus"))
        .expect("alias");
    assert_eq!(opus.model.id, "claude-opus-5-5");
    assert!(opus.model.capabilities.effort);
    let local = reg
        .resolve(&ModelRef::new("ollama", "qwen3:32b"))
        .expect("open list");
    assert_eq!(local.model.id, "qwen3:32b");
    // Anthropic's list is open so future Claude ids work with the default profile.
    let future = reg
        .resolve(&ModelRef::new("anthropic", "claude-9"))
        .expect("open list");
    assert_eq!(future.model.capabilities.context_window, 200_000);
    let mut strict = Registry::empty();
    let mut def = reg.provider("anthropic").expect("anthropic").clone();
    def.flags.open_model_list = false;
    strict.insert(def);
    assert!(matches!(
        strict.resolve(&ModelRef::new("anthropic", "claude-9")),
        Err(RegistryError::UnknownModel { .. })
    ));
    assert!(matches!(
        reg.resolve(&ModelRef::new("nope", "x")),
        Err(RegistryError::UnknownProvider(_))
    ));
}

#[test]
fn claude_engine_profiles_preserve_configured_context_windows() {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        r#"
[providers.custom-claude]
protocol = "claude-code"
base_url = ""
[providers.custom-claude.default_capabilities]
context_window = 1000000
max_output_tokens = 8000
[[providers.custom-claude.models]]
id = "large"
[providers.custom-claude.models.capabilities]
context_window = 1000000
max_output_tokens = 8000
[[providers.custom-claude.models]]
id = "small"
[providers.custom-claude.models.capabilities]
context_window = 32000
max_output_tokens = 4000
[[providers.anthropic.add_models]]
id = "api-large"
[providers.anthropic.add_models.capabilities]
context_window = 1000000
max_output_tokens = 8000
"#,
    )
    .expect("profiles");
    reg.apply_overrides(file).expect("apply profiles");
    let custom = reg.provider("custom-claude").expect("custom engine");
    assert_eq!(
        custom.model("large").unwrap().capabilities.context_window,
        1_000_000
    );
    assert_eq!(
        custom.model("small").unwrap().capabilities.context_window,
        32_000
    );
    assert_eq!(
        custom.default_capabilities.as_ref().unwrap().context_window,
        1_000_000
    );
    assert_eq!(
        reg.provider("anthropic")
            .unwrap()
            .model("api-large")
            .unwrap()
            .capabilities
            .context_window,
        1_000_000
    );

    let mut replacement = custom.clone();
    replacement.models[0].capabilities.context_window = 1_000_000;
    replacement
        .default_capabilities
        .as_mut()
        .unwrap()
        .context_window = 1_000_000;
    reg.insert(replacement);
    let replaced = reg.provider("custom-claude").unwrap();
    assert_eq!(
        replaced.model("large").unwrap().capabilities.context_window,
        1_000_000
    );
    assert_eq!(
        replaced
            .default_capabilities
            .as_ref()
            .unwrap()
            .context_window,
        1_000_000
    );
}

#[test]
fn overrides_merge_and_add_providers() {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        r#"
[providers.anthropic]
base_url = "https://proxy.example/anthropic"
[[providers.anthropic.add_models]]
id = "claude-custom"
name = "Custom"
[providers.anthropic.add_models.capabilities]
context_window = 50000
max_output_tokens = 4000

[providers.mylocal]
protocol = "openai-chat-completions"
base_url = "http://10.0.0.5:8000/v1"
[providers.mylocal.auth]
api_key_env = ["MYLOCAL_KEY"]
[providers.mylocal.flags]
stream_options = false
[[providers.mylocal.models]]
id = "big-model"
[providers.mylocal.models.capabilities]
context_window = 32768
max_output_tokens = 8192
tools = true
"#,
    )
    .expect("toml");
    reg.apply_overrides(file).expect("apply");
    let anth = reg.provider("anthropic").expect("anthropic");
    assert_eq!(anth.base_url, "https://proxy.example/anthropic");
    assert!(anth.model("claude-opus-5-5").is_some());
    assert_eq!(
        anth.model("claude-custom")
            .map(|m| m.capabilities.context_window),
        Some(50000)
    );
    let local = reg.provider("mylocal").expect("mylocal");
    assert_eq!(local.protocol, Protocol::OpenaiChatCompletions);
    assert!(!local.flags.stream_options);
    assert_eq!(local.auth.api_key_env, vec!["MYLOCAL_KEY"]);
    assert_eq!(
        reg.resolve(&ModelRef::new("mylocal", "big-model"))
            .expect("m")
            .model
            .capabilities
            .context_window,
        32768
    );

    let bad: ProvidersFile =
        toml::from_str("[providers.incomplete]\nname = \"x\"\n").expect("toml");
    assert!(matches!(
        reg.apply_overrides(bad),
        Err(RegistryError::Config { .. })
    ));
}

/// `FamilyOverride`'s untagged variant is subtle: `family = "claude"`
/// must parse as the family and `family = "none"` must clear an
/// inherited one, rather than one swallowing the other.
#[test]
fn a_family_override_names_a_family_or_clears_it() {
    let mut reg = Registry::builtin();
    let file: ProvidersFile =
        toml::from_str("[providers.groq]\nfamily = \"claude\"\n").expect("toml");
    reg.apply_overrides(file).expect("apply");
    assert_eq!(
        reg.provider("groq").expect("groq").family,
        Some(ModelFamily::Claude)
    );

    let cleared: ProvidersFile =
        toml::from_str("[providers.groq]\nfamily = \"none\"\n").expect("toml");
    reg.apply_overrides(cleared).expect("apply");
    assert_eq!(
        reg.provider("groq").expect("groq").family,
        None,
        "`none` clears the family, it is not a family called none"
    );

    assert!(
        toml::from_str::<ProvidersFile>("[providers.groq]\nfamily = \"gemini\"\n").is_err(),
        "an unknown family must fail to load, never become no profile at all"
    );
}

#[test]
fn disabled_provider_is_reported() {
    let mut reg = Registry::builtin();
    let file: ProvidersFile = toml::from_str("[providers.groq]\nenabled = false\n").expect("toml");
    reg.apply_overrides(file).expect("apply");
    assert!(matches!(
        reg.resolve(&ModelRef::new("groq", "x")),
        Err(RegistryError::DisabledProvider(_))
    ));
}

/// `sonnet`, `fable` and `haiku` were primary ids on `claude-code` before they
/// became aliases of pinned versions. A `providers.toml` written against the old
/// ids must still override those models, not append an entry lookup never reaches.
#[test]
fn an_add_models_entry_keyed_by_an_alias_overrides_the_model_it_names() {
    let mut reg = Registry::builtin();
    let before = reg.provider("claude-code").unwrap().models.len();
    let file: ProvidersFile = toml::from_str(
        r#"
[[providers.claude-code.add_models]]
id = "sonnet"
name = "Sonnet override"
aliases = ["my-sonnet"]
[providers.claude-code.add_models.capabilities]
context_window = 1000000
max_output_tokens = 8000
cost = { input_per_mtok = 7.0, output_per_mtok = 9.0 }
"#,
    )
    .expect("toml");
    reg.apply_overrides(file).expect("apply");
    let cc = reg.provider("claude-code").unwrap();
    assert_eq!(cc.models.len(), before, "overridden in place, not appended");
    for id in [
        "sonnet",
        "claude-sonnet-5-5",
        "sonnet-5-5",
        "sonnet-5.5",
        "my-sonnet",
    ] {
        let m = cc.model(id).unwrap_or_else(|| panic!("{id} resolves"));
        assert_eq!(m.id, "claude-sonnet-5-5", "{id} keeps the pinned id");
        assert_eq!(m.name, "Sonnet override", "{id}");
        assert_eq!(m.capabilities.context_window, 1_000_000, "{id}");
        assert_eq!(m.capabilities.cost.as_ref().unwrap().input_per_mtok, 7.0);
    }
    let haiku = cc.model("haiku").unwrap();
    assert_eq!(
        (haiku.id.as_str(), haiku.name.as_str()),
        ("claude-haiku-5-5", "Claude Haiku 5.5"),
        "other models are untouched"
    );
}

#[test]
fn overrides_reject_colliding_model_ids_and_aliases() {
    for (settings, first, second, key) in [
        (
            r#"add_models = [{id = "custom", aliases = ["astra"]}]"#,
            "gpt-6-astra",
            "custom",
            "astra",
        ),
        (
            r#"add_models = [{id = "custom", aliases = ["gpt-6-astra"]}]"#,
            "gpt-6-astra",
            "custom",
            "gpt-6-astra",
        ),
        (
            r#"models = [{id = "first", aliases = ["shared"]}, {id = "second", aliases = ["shared"]}]"#,
            "first",
            "second",
            "shared",
        ),
        (
            r#"models = [{id = "same"}, {id = "same"}]"#,
            "same",
            "same",
            "same",
        ),
        (
            r#"models = [{id = "first", aliases = ["second"]}, {id = "second"}]"#,
            "first",
            "second",
            "second",
        ),
    ] {
        let mut registry = Registry::builtin();
        let file = toml::from_str(&format!("[providers.openai]\n{settings}\n")).unwrap();
        let RegistryError::Config { reason, .. } =
            registry.apply_overrides(file).expect_err(settings)
        else {
            panic!("expected a config error")
        };
        for name in ["openai", first, second, key] {
            assert!(reason.contains(name), "missing {name}: {reason}");
        }
    }
}

#[test]
fn model_aliases_are_unique_within_each_provider() {
    let mut registry = Registry::builtin();
    let file = toml::from_str(
        r#"
[providers.openai]
models = [{id = "first", aliases = ["shared"]}]
[providers.anthropic]
models = [{id = "second", aliases = ["shared"]}]
"#,
    )
    .unwrap();
    registry.apply_overrides(file).unwrap();
    assert_eq!(
        registry
            .provider("openai")
            .unwrap()
            .model("shared")
            .unwrap()
            .id,
        "first"
    );
    assert_eq!(
        registry
            .provider("anthropic")
            .unwrap()
            .model("shared")
            .unwrap()
            .id,
        "second"
    );
}

#[test]
fn a_model_can_include_its_own_id_as_an_alias() {
    let mut registry = Registry::builtin();
    let file = toml::from_str(
        r#"
[providers.openai]
add_models = [{id = "custom", aliases = ["custom", "short"]}]
"#,
    )
    .unwrap();
    registry.apply_overrides(file).unwrap();
    let provider = registry.provider("openai").unwrap();
    for name in ["custom", "short"] {
        assert_eq!(provider.model(name).unwrap().id, "custom");
    }
}

#[test]
fn new_provider_model_aliases_cannot_collide() {
    let mut registry = Registry::builtin();
    let file = toml::from_str(
        r#"
[providers.custom]
protocol = "openai-chat-completions"
base_url = "https://example.com/v1"
models = [{id = "first", aliases = ["shared"]}, {id = "second", aliases = ["shared"]}]
"#,
    )
    .unwrap();
    let error = registry.apply_overrides(file).unwrap_err().to_string();
    for name in ["custom", "first", "second", "shared"] {
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn provider_cost_overrides_parse_flat_and_tiered_prices() {
    use aether_core::model::Usage;
    for (cost, expected) in [
        ("{ input_per_mtok = 1.0, output_per_mtok = 2.0 }", 0.120001),
        (
            "{ input_per_mtok = 1.0, output_per_mtok = 2.0, above_threshold = { prompt_tokens = 100000, input_per_mtok = 5.0, output_per_mtok = 10.0, cache_read_per_mtok = 0.5, cache_write_per_mtok = 6.25 } }",
            1.275005,
        ),
    ] {
        let mut registry = Registry::builtin();
        let file = toml::from_str(&format!(
            "[providers.anthropic]\nadd_models = [{{ id = \"haiku\", capabilities = {{ context_window = 1000000, max_output_tokens = 8192, cost = {cost} }} }}]\n"
        )).unwrap();
        registry.apply_overrides(file).unwrap();
        let rate = registry
            .provider("anthropic")
            .unwrap()
            .model("haiku")
            .unwrap()
            .capabilities
            .cost
            .unwrap();
        let usage = Usage {
            input_tokens: 100_001,
            cache_read_tokens: 100_000,
            cache_write_tokens: 100_000,
            output_tokens: 10_000,
            ..Default::default()
        };
        assert!((rate.cost_of(&usage) - expected).abs() < 1e-12, "{cost}");
    }
}

#[test]
fn duplicate_aliases_on_one_model_do_not_block_the_registry() {
    let mut registry = Registry::builtin();
    let file: ProvidersFile = toml::from_str(
        "[providers.openai]\nmodels = [{id = 'custom', aliases = ['short', 'short', 'custom']}]\n",
    )
    .unwrap();
    registry.apply_overrides(file).unwrap();
    assert_eq!(
        registry
            .resolve(&ModelRef::new("openai", "short"))
            .unwrap()
            .model
            .id,
        "custom"
    );
}
