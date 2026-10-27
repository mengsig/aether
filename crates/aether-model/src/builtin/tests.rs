use super::*;

#[test]
fn claude_cli_and_api_models_share_the_full_window() {
    let all = providers();
    let cli = all
        .iter()
        .find(|provider| provider.id == "claude-code")
        .unwrap();
    let api = all
        .iter()
        .find(|provider| provider.id == "anthropic")
        .unwrap();
    for (cli_id, api_id) in [
        ("fable", "claude-fable-5-1"),
        ("opus", "claude-opus-5-5"),
        ("sonnet", "claude-sonnet-5-5"),
        ("haiku", "claude-haiku-5-5"),
    ] {
        assert_eq!(
            cli.model(cli_id).unwrap().capabilities.context_window,
            1_000_000
        );
        assert_eq!(
            api.model(api_id).unwrap().capabilities.context_window,
            1_000_000
        );
    }
}

/// The `[1m]` ids and `-1m` aliases stay out of the catalogue: every built-in
/// model already has the full window, and an open list would accept any id.
#[test]
fn no_1m_variant_ids_are_built_in_for_claude_code() {
    let all = providers();
    let cli = all
        .iter()
        .find(|provider| provider.id == "claude-code")
        .unwrap();
    assert!(!cli.models.is_empty());
    for model in &cli.models {
        assert!(!model.id.ends_with("[1m]"), "built-in id {}", model.id);
        assert!(
            model.aliases.iter().all(|alias| !alias.ends_with("-1m")),
            "built-in aliases of {}: {:?}",
            model.id,
            model.aliases
        );
        assert_eq!(model.capabilities.context_window, 1_000_000, "{}", model.id);
    }
    let default_window = cli.default_capabilities.as_ref().unwrap().context_window;
    assert_eq!(default_window, 1_000_000);
    assert!(
        !cli.flags.open_model_list,
        "an open list would accept any `[1m]` id without an explicit edit"
    );
}

/// A subscription turn still has to cost something a report or a budget
/// can bind to, and it must be the same number the API path would give.
#[test]
fn claude_code_models_are_priced_like_the_api_models_behind_them() {
    let all = providers();
    let engine = all.iter().find(|p| p.id == "claude-code").expect("engine");
    let api = all.iter().find(|p| p.id == "anthropic").expect("api");
    for (engine_id, api_id) in [
        ("opus", "claude-opus-5-5"),
        ("sonnet", "claude-sonnet-5-5"),
        ("haiku", "claude-haiku-5-5"),
        ("fable", "claude-fable-5-1"),
    ] {
        let priced = engine
            .model(engine_id)
            .unwrap_or_else(|| panic!("claude-code/{engine_id}"))
            .capabilities
            .cost;
        let reference = api
            .model(api_id)
            .unwrap_or_else(|| panic!("anthropic/{api_id}"))
            .capabilities
            .cost;
        assert_eq!(priced, reference, "claude-code/{engine_id}");
        assert!(priced.is_some(), "claude-code/{engine_id} must be priced");
    }
    assert!(
        engine
            .model("default")
            .expect("default")
            .capabilities
            .cost
            .is_none(),
        "`default` is whatever the CLI picks; pricing it would bill the wrong model"
    );
}

/// The `codex` engine route is a ChatGPT subscription: it carries no price of
/// its own, so its turns are estimated from the API model behind each slug.
/// The operator-supplied `openai-codex` endpoint bills per token and is priced.
#[test]
fn codex_engine_models_are_estimated_from_the_api_models_behind_them() {
    let registry = crate::registry::Registry::builtin();
    let all = providers();
    let api = all.iter().find(|p| p.id == "openai").expect("api");
    let engine = all.iter().find(|p| p.id == "codex").expect("codex");
    for model in &engine.models {
        assert_eq!(model.capabilities.cost, None, "codex/{}", model.id);
    }
    for id in ["gpt-6-astra", "gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-luna"] {
        let resolved = registry
            .resolve(&aether_core::model::ModelRef::new("codex", id))
            .unwrap_or_else(|_| panic!("codex/{id}"));
        let reference = api
            .model(id)
            .unwrap_or_else(|| panic!("openai/{id}"))
            .capabilities
            .cost;
        assert!(reference.is_some(), "openai/{id} must be priced");
        assert_eq!(registry.api_equivalent_price(&resolved), reference, "{id}");
    }
    // `default` is whatever the CLI picks; estimating it would bill the wrong model.
    let default = registry
        .resolve(&aether_core::model::ModelRef::new("codex", "default"))
        .expect("codex/default");
    assert_eq!(registry.api_equivalent_price(&default), None);

    let dialect = all
        .iter()
        .find(|p| p.id == "openai-codex")
        .expect("dialect");
    for id in ["gpt-6-astra", "gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-luna"] {
        let priced = dialect.model(id).expect(id).capabilities.cost;
        assert!(priced.is_some(), "openai-codex/{id} must be priced");
        assert_eq!(priced, api.model(id).expect(id).capabilities.cost, "{id}");
    }
    // No slug has a price of its own: a price is the API's or none.
    for model in &dialect.models {
        let reference = api.model(&model.id).and_then(|m| m.capabilities.cost);
        assert_eq!(
            model.capabilities.cost, reference,
            "openai-codex/{}",
            model.id
        );
    }
}

#[test]
fn builtin_ids_are_unique_and_anthropic_costs_match_the_sheet() {
    let all = providers();
    let mut ids: Vec<_> = all.iter().map(|p| p.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), all.len());
    let anth = all.iter().find(|p| p.id == "anthropic").expect("anthropic");
    let haiku = anth.model("haiku").expect("haiku 5.5").capabilities.clone();
    assert_eq!(
        (haiku.context_window, haiku.max_output_tokens),
        (1_000_000, 128_000)
    );
    assert_eq!(haiku.thinking, ThinkingSupport::Adaptive);
    assert!(haiku.effort);
    let cost = haiku.cost.expect("haiku 5.5 is priced");
    assert_eq!((cost.input_per_mtok, cost.output_per_mtok), (0.10, 0.50));
    let fable = anth
        .model("claude-fable-5-1")
        .expect("fable")
        .capabilities
        .cost
        .expect("cost");
    assert_eq!(fable.cache_read_per_mtok, 0.25);
    assert!(
        all.iter()
            .find(|p| p.id == "ollama")
            .expect("ollama")
            .auth
            .none
    );
}

#[test]
fn claude_one_hour_cache_writes_cost_twice_input_and_no_other_family_has_them() {
    let all = providers();
    let mut claude = 0;
    for provider in &all {
        let anthropic = matches!(provider.id.as_str(), "anthropic" | "claude-code");
        for model in &provider.models {
            let Some(cost) = model.capabilities.cost else {
                continue;
            };
            if !anthropic {
                assert_eq!(cost.cache_write_1h_per_mtok, None, "{}", model.id);
                continue;
            }
            claude += 1;
            assert_eq!(
                cost.cache_write_1h_per_mtok,
                Some(cost.input_per_mtok * 2.0),
                "{}",
                model.id
            );
            if let Some(tier) = cost.above_threshold {
                assert_eq!(
                    tier.cache_write_1h_per_mtok,
                    Some(tier.input_per_mtok * 2.0)
                );
            }
        }
    }
    assert_eq!(claude, 8, "four API models and four priced engine models");
    let haiku = all
        .iter()
        .find(|p| p.id == "claude-code")
        .and_then(|p| p.model("haiku"))
        .and_then(|m| m.capabilities.cost)
        .expect("claude-code/haiku is priced");
    assert_eq!(haiku.cache_write_1h_per_mtok, Some(0.20));
    assert_eq!(
        haiku
            .above_threshold
            .and_then(|t| t.cache_write_1h_per_mtok),
        Some(1.0)
    );
}

#[test]
fn current_openai_and_engine_models_resolve_by_alias() {
    let all = providers();
    let by_id = |id: &str| all.iter().find(|p| p.id == id).expect(id);
    let astra = by_id("openai").model("astra").expect("astra alias");
    assert_eq!(astra.id, "gpt-6-astra");
    let c = astra.capabilities.cost.expect("astra cost");
    assert_eq!((c.input_per_mtok, c.output_per_mtok), (10.0, 50.0));
    assert_eq!(astra.capabilities.context_window, 1_050_000);
    for (alias, id) in [
        ("astra", "gpt-6-astra"),
        ("terra", "gpt-6.1-terra"),
        ("gpt-5.6-terra", "gpt-6.1-terra"),
        ("sol", "gpt-6.1-sol"),
        ("luna", "gpt-6-luna"),
    ] {
        for provider in ["codex", "openai-codex", "openai", "openai-responses"] {
            let model = by_id(provider).model(alias);
            assert_eq!(model.expect(alias).id, id, "{provider}/{alias}");
        }
    }
    assert!(by_id("openai-codex").model("default").is_none());
    for (alias, id) in [
        ("fable-5-1", "claude-fable-5-1"),
        ("opus-5-5", "claude-opus-5-5"),
        ("opus-5.5", "claude-opus-5-5"),
        ("sonnet-5-5", "claude-sonnet-5-5"),
        ("sonnet-5.5", "claude-sonnet-5-5"),
        ("haiku-5-5", "claude-haiku-5-5"),
        ("haiku-5.5", "claude-haiku-5-5"),
    ] {
        for provider in ["claude-code", "anthropic"] {
            let model = by_id(provider).model(alias);
            assert_eq!(model.expect(alias).id, id, "{provider}/{alias}");
        }
    }
}

/// Superseded models are gone from the catalogue, aliases included: a stale
/// pin must fail loudly, never land silently on a successor.
#[test]
fn superseded_models_and_their_aliases_are_not_in_the_catalogue() {
    let all = providers();
    let by_id = |id: &str| all.iter().find(|p| p.id == id).expect(id);
    for (provider, retired) in [
        (
            "anthropic",
            &[
                "claude-opus-5",
                "opus-5",
                "claude-sonnet-5",
                "sonnet-5",
                "claude-haiku-4-5",
                "haiku-4-5",
                "claude-haiku-4-5-20251001",
                "claude-fable-5",
                "claude-opus-4-8",
                "claude-opus-4-7",
                "claude-opus-4-6",
                "claude-sonnet-4-6",
            ][..],
        ),
        (
            "claude-code",
            &[
                "claude-opus-5",
                "opus-5",
                "claude-sonnet-5",
                "sonnet-5",
                "claude-haiku-4-5",
                "haiku-4-5",
                "claude-haiku-4-5-20251001",
            ][..],
        ),
        (
            "codex",
            &[
                "gpt-6-sol",
                "gpt-5.6-sol",
                "gpt-5.6",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.3-codex-spark",
                "spark",
            ][..],
        ),
    ] {
        for name in retired {
            assert!(
                by_id(provider).model(name).is_none(),
                "{provider}/{name} must not resolve"
            );
        }
    }
    for provider in ["openai", "openai-responses", "openai-codex"] {
        for name in [
            "gpt-6-sol",
            "gpt-5.6-sol",
            "gpt-5.6",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.3-codex-spark",
            "spark",
        ] {
            assert!(
                by_id(provider).model(name).is_none(),
                "{provider}/{name} must not resolve"
            );
        }
    }
}

/// The exact lineup each first-party provider ships, in preference order.
#[test]
fn first_party_providers_ship_only_the_current_lineup() {
    let all = providers();
    let ids = |id: &str| -> Vec<String> {
        let provider = all.iter().find(|p| p.id == id).expect(id);
        provider.models.iter().map(|m| m.id.clone()).collect()
    };
    assert_eq!(
        ids("anthropic"),
        [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1"
        ]
    );
    assert_eq!(
        ids("claude-code"),
        [
            "claude-opus-5-5",
            "default",
            "claude-fable-5-1",
            "claude-sonnet-5-5",
            "claude-haiku-5-5"
        ]
    );
    assert_eq!(
        ids("codex"),
        [
            "gpt-6-astra",
            "gpt-6.1-terra",
            "gpt-6.1-sol",
            "gpt-6-luna",
            "default"
        ]
    );
    for id in ["openai", "openai-responses", "openai-codex"] {
        assert_eq!(
            ids(id),
            ["gpt-6-astra", "gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-luna"],
            "{id}"
        );
    }
}

#[test]
fn gpt6_preference_order_and_prices_are_consistent() {
    let registry = crate::registry::Registry::builtin();
    for id in ["codex", "openai-codex", "openai", "openai-responses"] {
        let provider = registry.provider(id).unwrap();
        assert_eq!(
            provider
                .models
                .iter()
                .take(4)
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-6-astra", "gpt-6.1-terra", "gpt-6.1-sol", "gpt-6-luna"]
        );
        // Cache reads are 10% of input, except GPT-6.1's 5%.
        for (alias, expected, input, output, write, read) in [
            ("astra", "gpt-6-astra", 10.0, 50.0, 12.5, 1.0),
            ("terra", "gpt-6.1-terra", 1.0, 5.0, 1.25, 0.05),
            ("sol", "gpt-6.1-sol", 2.0, 10.0, 2.5, 0.10),
            ("luna", "gpt-6-luna", 0.10, 0.50, 0.125, 0.01),
        ] {
            let model = provider.model(alias).unwrap();
            assert_eq!(model.id, expected);
            // The Codex dialect ships disabled, so only its table is readable.
            if provider.enabled {
                let resolved = registry
                    .resolve(&aether_core::model::ModelRef::new(id, alias))
                    .unwrap();
                assert_eq!(resolved.model, *model, "{id}/{alias}");
            }
            // The codex engine is estimated from its API twin, not priced.
            let price = model
                .capabilities
                .cost
                .or_else(|| {
                    let resolved = registry
                        .resolve(&aether_core::model::ModelRef::new(id, alias))
                        .ok()?;
                    registry.api_equivalent_price(&resolved)
                })
                .unwrap_or_else(|| panic!("{id}/{alias} is priced"));
            assert_eq!(
                (
                    price.input_per_mtok,
                    price.output_per_mtok,
                    price.cache_write_per_mtok
                ),
                (input, output, write)
            );
            assert!(
                (price.cache_read_per_mtok - read).abs() < 1e-10,
                "{id}/{alias}"
            );
        }
    }
}

#[test]
fn opus_55_is_preferred_with_launch_pricing() {
    for provider in providers()
        .into_iter()
        .filter(|p| matches!(p.id.as_str(), "anthropic" | "claude-code"))
    {
        assert_eq!(provider.models[0].id, "claude-opus-5-5");
        for alias in ["opus", "opus-5-5", "opus-5.5", "claude-opus-5-5"] {
            let model = provider.model(alias).unwrap();
            assert_eq!(model.id, "claude-opus-5-5");
            let price = model.capabilities.cost.unwrap();
            assert_eq!(price.input_per_mtok, 4.0);
            assert_eq!(price.output_per_mtok, 20.0);
            assert_eq!(price.cache_read_per_mtok, 0.20);
            assert_eq!(price.cache_write_per_mtok, 5.0);
        }
        assert!(provider.model("fable").is_some());
    }
}

/// Both Claude paths agree that `sonnet` means Sonnet 5.5.
#[test]
fn sonnet_55_owns_the_sonnet_alias() {
    for provider in providers()
        .into_iter()
        .filter(|p| matches!(p.id.as_str(), "anthropic" | "claude-code"))
    {
        let id = &provider.id;
        for alias in ["sonnet", "sonnet-5-5", "sonnet-5.5", "claude-sonnet-5-5"] {
            let model = provider
                .model(alias)
                .unwrap_or_else(|| panic!("{id}/{alias}"));
            assert_eq!(model.name, "Claude Sonnet 5.5", "{id}/{alias}");
            assert!(model.capabilities.effort, "{id}/{alias}");
            assert_eq!(model.capabilities.max_output_tokens, 128_000);
            let price = model.capabilities.cost.expect("sonnet 5.5 is priced");
            assert_eq!(
                (
                    price.input_per_mtok,
                    price.output_per_mtok,
                    price.cache_read_per_mtok,
                    price.cache_write_per_mtok,
                    price.cache_write_1h_per_mtok
                ),
                // Cache reads at 0.05x input, as on the Anthropic price sheet.
                (2.0, 10.0, 0.10, 2.5, Some(4.0)),
                "{id}/{alias}"
            );
        }
    }
}

/// `--model` receives the canonical id, so each family's short name must land
/// on an explicit version: a floating CLI alias could pick an older model.
#[test]
fn claude_code_family_aliases_pin_the_newest_explicit_versions() {
    let all = providers();
    let engine = all.iter().find(|p| p.id == "claude-code").expect("engine");
    for (family, pinned, name) in [
        ("opus", "claude-opus-5-5", "Claude Opus 5.5"),
        ("sonnet", "claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ("fable", "claude-fable-5-1", "Claude Fable 5.1"),
        ("haiku", "claude-haiku-5-5", "Claude Haiku 5.5"),
    ] {
        for requested in [family, pinned] {
            let model = engine
                .model(requested)
                .unwrap_or_else(|| panic!("claude-code/{requested}"));
            assert_eq!(
                (model.id.as_str(), model.name.as_str()),
                (pinned, name),
                "claude-code/{requested}"
            );
        }
        assert!(
            engine.models.iter().all(|m| m.id != family),
            "`{family}` must stay an alias, never the id passed to --model"
        );
    }
}

/// `(family, version)` of a GPT id such as `gpt-6.1-sol` or
/// `gpt-5.3-codex-spark`; `None` for an unversioned or familyless id.
fn gpt_family_version(id: &str) -> Option<(&str, Vec<u32>)> {
    let rest = id.strip_prefix("gpt-")?;
    let (version, tail) = rest.split_once('-')?;
    let family = tail.rsplit('-').next()?;
    let version = version
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u32>, _>>()
        .ok()?;
    Some((family, version))
}

/// Each OpenAI family's short name must follow its newest explicit version, so
/// adding e.g. a gpt-6.1-astra without moving `astra` fails here.
#[test]
fn openai_family_short_names_alias_the_newest_version_on_every_provider() {
    let all = providers();
    for provider_id in ["codex", "openai-codex", "openai", "openai-responses"] {
        let families = ["astra", "luna", "sol", "terra"];
        let provider = all.iter().find(|p| p.id == provider_id).expect(provider_id);
        let mut newest: BTreeMap<&str, (Vec<u32>, &str)> = BTreeMap::new();
        for model in &provider.models {
            let Some((family, version)) = gpt_family_version(&model.id) else {
                continue;
            };
            let entry = newest.entry(family).or_insert((version.clone(), &model.id));
            if version > entry.0 {
                *entry = (version, &model.id);
            }
        }
        assert_eq!(
            newest.keys().copied().collect::<Vec<_>>(),
            families,
            "{provider_id} serves exactly these families"
        );
        for (family, (_, id)) in &newest {
            let model = provider
                .model(family)
                .unwrap_or_else(|| panic!("{provider_id}/{family} is missing"));
            assert_eq!(&model.id, id, "{provider_id}/{family}");
        }
    }
}

#[test]
fn ordering_helper_ranks_point_releases_above_their_major() {
    let v = |id| gpt_family_version(id).unwrap();
    assert_eq!(v("gpt-6.1-sol"), ("sol", vec![6, 1]));
    assert_eq!(v("gpt-5.3-codex-spark"), ("spark", vec![5, 3]));
    assert!(v("gpt-6.1-sol").1 > v("gpt-6-sol").1);
    assert!(v("gpt-6-sol").1 > v("gpt-5.6-sol").1);
    assert!(v("gpt-5.10-sol").1 > v("gpt-5.9-sol").1);
    assert_eq!(gpt_family_version("gpt-5.5"), None);
    assert_eq!(gpt_family_version("default"), None);
}

/// GPT-6.1 Sol and Terra run at high effort unless asked otherwise, on every
/// route that serves them; no other model gains a default.
#[test]
fn gpt_6_1_defaults_to_high_effort_and_an_explicit_effort_wins() {
    use aether_core::model::Effort;
    for provider in providers() {
        for model in &provider.models {
            let gpt_6_1 = matches!(model.id.as_str(), "gpt-6.1-sol" | "gpt-6.1-terra");
            let expected = gpt_6_1.then_some(Effort::High);
            assert_eq!(
                model.default_effort, expected,
                "{}/{}",
                provider.id, model.id
            );
        }
    }
    let codex = providers().into_iter().find(|p| p.id == "codex").unwrap();
    for alias in ["sol", "terra"] {
        let model = codex.model(alias).unwrap();
        assert_eq!(model.effort_for(None), Some(Effort::High), "{alias}");
        for explicit in [Effort::Low, Effort::Medium, Effort::XHigh, Effort::Max] {
            assert_eq!(model.effort_for(Some(explicit)), Some(explicit));
        }
    }
    assert_eq!(codex.model("luna").unwrap().effort_for(None), None);
}

/// Lookup takes the first match, so a name two models claim would silently
/// shadow the later one.
#[test]
fn no_builtin_model_name_is_claimed_twice_within_a_provider() {
    for provider in providers() {
        let mut seen = std::collections::BTreeSet::new();
        for model in &provider.models {
            for name in std::iter::once(&model.id).chain(&model.aliases) {
                assert!(
                    seen.insert(name.as_str()),
                    "{}/{name} is claimed by two models",
                    provider.id
                );
            }
        }
    }
}

/// VISION §1.6: a subscription is used only through the official client
/// that owns it. The Codex dialect therefore ships unreachable, and the
/// `codex` engine is the route a ChatGPT subscription takes.
#[test]
fn the_codex_dialect_ships_inert_and_the_engine_owns_the_subscription() {
    let all = providers();
    let by_id = |id: &str| all.iter().find(|p| p.id == id).expect(id);
    let dialect = by_id("openai-codex");
    assert!(!dialect.enabled, "the Codex dialect must ship disabled");
    assert!(
        dialect.base_url.is_empty(),
        "the Codex dialect must ship without a subscription endpoint"
    );
    assert_eq!(
        dialect.auth.oauth, None,
        "the Codex dialect must not offer a subscription login"
    );
    assert!(
        !all.iter().any(|p| {
            p.enabled
                && !p.protocol.is_engine()
                && p.auth.oauth.as_ref().is_some_and(|o| o.is_subscription())
        }),
        "no shipped HTTP adapter may sign into a subscription"
    );
    let engine = by_id("codex");
    assert!(engine.enabled && engine.protocol.is_engine());
}

#[test]
fn haiku_price_tables_select_long_prompt_tier_per_request() {
    use aether_core::model::Usage;
    for provider in providers()
        .into_iter()
        .filter(|p| matches!(p.id.as_str(), "anthropic" | "claude-code"))
    {
        let rate = provider.model("haiku").unwrap().capabilities.cost.unwrap();
        for (input, read, write, multiplier) in [
            (79_999, 10_000, 10_000, 1.0),
            (80_000, 10_000, 10_000, 1.0),
            (80_001, 10_000, 10_000, 5.0),
            (100_001, 0, 0, 5.0),
            (0, 100_001, 0, 5.0),
            (0, 0, 100_001, 5.0),
        ] {
            let usage = Usage {
                input_tokens: input,
                output_tokens: 10_000,
                cache_read_tokens: read,
                cache_write_tokens: write,
                ..Default::default()
            };
            let expected =
                (input as f64 * 0.10 + 10_000.0 * 0.50 + read as f64 * 0.01 + write as f64 * 0.125)
                    / 1_000_000.0
                    * multiplier;
            assert!(
                (rate.cost_of(&usage) - expected).abs() < 1e-12,
                "{}: {usage:?}",
                provider.id
            );
        }
    }
}
