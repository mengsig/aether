//! Built-in provider and model definitions. Users override or extend them in
//! `providers.toml`; nothing here is special to the runtime.

use std::collections::BTreeMap;

use aether_core::model::{CapabilityProfile, Effort, ModelCost, ModelCostTier, ThinkingSupport};

use crate::auth::{AuthMethods, CredentialImport, OAuthKind};
use crate::profile::ModelFamily;
use crate::registry::{ModelDef, Protocol, ProviderDef, ProviderFlags};
use crate::tier::ModelTier;

fn cost(input: f64, output: f64) -> ModelCost {
    // Cache writes 1.25x input, cache reads 0.1x input.
    ModelCost {
        input_per_mtok: input,
        output_per_mtok: output,
        cache_read_per_mtok: input * 0.1,
        cache_write_per_mtok: input * 1.25,
        cache_write_1h_per_mtok: None,
        above_threshold: None,
    }
}

// https://platform.claude.com/docs/en/about-claude/pricing#prompt-caching:
// 5-minute writes 1.25x input, 1-hour writes 2x.
fn claude_cost(input: f64, output: f64) -> ModelCost {
    ModelCost {
        cache_write_1h_per_mtok: Some(input * 2.0),
        ..cost(input, output)
    }
}

/// Fable prices like the rest of the family but with a cheaper cache read.
/// Shared with `claude-code/fable` so the two can never drift apart.
fn fable_cost() -> ModelCost {
    ModelCost {
        cache_read_per_mtok: 0.25,
        ..claude_cost(10.0, 50.0)
    }
}

// https://www.anthropic.com/claude-opus-5-5 — cache reads are 5% of input.
fn opus_cost() -> ModelCost {
    ModelCost {
        cache_read_per_mtok: 0.20,
        ..claude_cost(4.0, 20.0)
    }
}

// Like Opus 5.5, cache reads are 5% of input, not 10%.
fn sonnet_cost() -> ModelCost {
    ModelCost {
        cache_read_per_mtok: 0.10,
        ..claude_cost(2.0, 10.0)
    }
}

// https://www.anthropic.com/claude-haiku-5-5 — over 100K prompt tokens,
// every token in the request bills at 5x.
fn haiku_cost() -> ModelCost {
    ModelCost {
        above_threshold: Some(ModelCostTier {
            prompt_tokens: 100_000,
            input_per_mtok: 0.50,
            output_per_mtok: 2.50,
            cache_read_per_mtok: 0.05,
            cache_write_per_mtok: 0.625,
            cache_write_1h_per_mtok: Some(1.0),
        }),
        ..claude_cost(0.10, 0.50)
    }
}

fn claude(
    context: u32,
    max_out: u32,
    thinking: ThinkingSupport,
    effort: bool,
    cost: ModelCost,
) -> CapabilityProfile {
    CapabilityProfile {
        context_window: context,
        max_output_tokens: max_out,
        tools: true,
        parallel_tools: true,
        vision: true,
        thinking,
        effort,
        prompt_caching: true,
        structured_output: true,
        strict_tools: true,
        cost: Some(cost),
        extensions: BTreeMap::new(),
        compaction_tokens: None,
    }
}

fn anthropic() -> ProviderDef {
    let adaptive =
        |ctx: u32, out: u32, c: ModelCost| claude(ctx, out, ThinkingSupport::Adaptive, true, c);
    ProviderDef {
        id: "anthropic".into(),
        name: "Anthropic".into(),
        protocol: Protocol::AnthropicMessages,
        base_url: "https://api.anthropic.com".into(),
        auth: AuthMethods::api_key(["ANTHROPIC_API_KEY"])
            .with_bearer_env(["ANTHROPIC_AUTH_TOKEN"])
            .with_import(CredentialImport::AnthropicProfile { profile: None }),
        headers: BTreeMap::new(),
        models: vec![
            ModelDef::new(
                "claude-opus-5-5",
                "Claude Opus 5.5",
                adaptive(1_000_000, 128_000, opus_cost()),
            )
            .with_aliases(["opus", "opus-5-5", "opus-5.5"])
            .with_tier(ModelTier::Strong),
            ModelDef::new(
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                adaptive(1_000_000, 128_000, sonnet_cost()),
            )
            .with_aliases(["sonnet", "sonnet-5-5", "sonnet-5.5"])
            .with_tier(ModelTier::Balanced),
            ModelDef::new(
                "claude-haiku-5-5",
                "Claude Haiku 5.5",
                adaptive(1_000_000, 128_000, haiku_cost()),
            )
            .with_aliases(["haiku", "haiku-5-5", "haiku-5.5"])
            .with_tier(ModelTier::Fast),
            ModelDef::new(
                "claude-fable-5-1",
                "Claude Fable 5.1",
                adaptive(1_000_000, 128_000, fable_cost()),
            )
            .with_aliases(["fable", "fable-5-1"])
            .with_tier(ModelTier::Frontier),
        ],
        default_capabilities: Some(claude(
            1_000_000,
            32_000,
            ThinkingSupport::None,
            false,
            claude_cost(3.0, 15.0),
        )),
        flags: ProviderFlags {
            open_model_list: true,
            ..Default::default()
        },
        max_concurrent: None,
        family: Some(ModelFamily::Claude),
        enabled: true,
        usage_probe: Default::default(),
    }
}

fn openai_like(context: u32, max_out: u32, effort: bool) -> CapabilityProfile {
    openai_priced(context, max_out, effort, None)
}

fn openai_priced(
    context: u32,
    max_out: u32,
    effort: bool,
    cost: Option<ModelCost>,
) -> CapabilityProfile {
    CapabilityProfile {
        context_window: context,
        max_output_tokens: max_out,
        tools: true,
        parallel_tools: true,
        vision: true,
        thinking: if effort {
            ThinkingSupport::Adaptive
        } else {
            ThinkingSupport::None
        },
        effort,
        prompt_caching: true,
        structured_output: true,
        strict_tools: true,
        cost,
        extensions: BTreeMap::new(),
        compaction_tokens: None,
    }
}

/// Operator's choice: GPT-6.1 Sol is strong and well priced at high effort,
/// but Codex defaults it to low. Shared by every provider that serves it.
const GPT_6_1_SOL_EFFORT: Effort = Effort::High;

/// Current OpenAI API models. Context and output limits are the API's;
/// prices are per million tokens from the model pages (September 2026).
/// Each family's short name is an alias of its newest explicit version.
fn openai_current_models() -> Vec<ModelDef> {
    // GPT-6 has explicit cache-write billing at 1.25x input.
    let gpt6 = |input, output| openai_priced(1_050_000, 128_000, true, Some(cost(input, output)));
    // GPT-6.1 Sol's cache reads are 5% of input, not GPT-6's 10%.
    let gpt61_sol = openai_priced(
        1_050_000,
        128_000,
        true,
        Some(ModelCost {
            cache_read_per_mtok: 0.10,
            ..cost(2.0, 10.0)
        }),
    );
    vec![
        ModelDef::new("gpt-6-astra", "GPT-6 Astra", gpt6(10.0, 50.0))
            .with_aliases(["astra"])
            .with_tier(ModelTier::Strong),
        ModelDef::new("gpt-6.1-sol", "GPT-6.1 Sol", gpt61_sol)
            .with_aliases(["sol"])
            .with_default_effort(GPT_6_1_SOL_EFFORT)
            .with_tier(ModelTier::Balanced),
        ModelDef::new("gpt-6-luna", "GPT-6 Luna", gpt6(0.10, 0.50))
            .with_aliases(["luna"])
            .with_tier(ModelTier::Fast),
    ]
}

fn openai() -> ProviderDef {
    ProviderDef {
        id: "openai".into(),
        name: "OpenAI".into(),
        protocol: Protocol::OpenaiChatCompletions,
        base_url: "https://api.openai.com/v1".into(),
        auth: AuthMethods::api_key(["OPENAI_API_KEY"]),
        headers: BTreeMap::new(),
        models: openai_current_models(),
        default_capabilities: Some(openai_like(128_000, 16_000, false)),
        flags: ProviderFlags {
            max_completion_tokens: true,
            open_model_list: true,
            ..Default::default()
        },
        max_concurrent: None,
        family: Some(ModelFamily::Gpt),
        enabled: true,
        usage_probe: Default::default(),
    }
}

/// The Responses API on the same account and models as `openai`. Kept a
/// separate provider so an existing `openai/...` reference keeps its protocol.
fn openai_responses() -> ProviderDef {
    ProviderDef {
        id: "openai-responses".into(),
        name: "OpenAI (Responses)".into(),
        protocol: Protocol::OpenaiResponses,
        base_url: "https://api.openai.com/v1".into(),
        auth: AuthMethods::api_key(["OPENAI_API_KEY"]),
        headers: BTreeMap::new(),
        models: openai_current_models(),
        default_capabilities: Some(openai_like(128_000, 16_000, false)),
        flags: ProviderFlags {
            open_model_list: true,
            ..Default::default()
        },
        max_concurrent: None,
        family: Some(ModelFamily::Gpt),
        enabled: true,
        usage_probe: Default::default(),
    }
}

/// The Responses protocol against a Codex backend. Ships inert - disabled, no
/// endpoint, no login - because VISION §1.6 keeps a ChatGPT subscription on
/// the official client that owns it: `codex` (the CLI engine) is the shipped
/// subscription route. Only an operator who supplies their own `base_url` and
/// credential in `providers.toml` reaches this dialect.
fn openai_codex() -> ProviderDef {
    ProviderDef {
        id: "openai-codex".into(),
        name: "OpenAI Codex (operator-configured endpoint)".into(),
        protocol: Protocol::OpenaiCodexResponses,
        base_url: String::new(),
        auth: AuthMethods::none(),
        headers: BTreeMap::new(),
        models: priced_codex_models()
            .into_iter()
            .filter(|m| m.id != "default")
            .collect(),
        default_capabilities: Some(openai_like(CODEX_CONTEXT, 128_000, true)),
        flags: ProviderFlags {
            open_model_list: true,
            ..Default::default()
        },
        max_concurrent: None,
        family: Some(ModelFamily::Codex),
        enabled: false,
        usage_probe: Default::default(),
    }
}

fn compat(
    id: &str,
    name: &str,
    base_url: &str,
    envs: &[&str],
    models: Vec<ModelDef>,
    default: CapabilityProfile,
) -> ProviderDef {
    ProviderDef {
        id: id.into(),
        name: name.into(),
        protocol: Protocol::OpenaiChatCompletions,
        base_url: base_url.into(),
        auth: if envs.is_empty() {
            AuthMethods::none()
        } else {
            AuthMethods::api_key(envs.iter().copied())
        },
        headers: BTreeMap::new(),
        models,
        default_capabilities: Some(default),
        flags: ProviderFlags {
            open_model_list: true,
            ..Default::default()
        },
        max_concurrent: None,
        family: None,
        enabled: true,
        usage_probe: Default::default(),
    }
}

fn local_profile() -> CapabilityProfile {
    CapabilityProfile {
        context_window: 32_768,
        max_output_tokens: 8_192,
        tools: true,
        parallel_tools: true,
        vision: false,
        thinking: ThinkingSupport::None,
        effort: false,
        prompt_caching: false,
        structured_output: false,
        strict_tools: false,
        cost: Some(ModelCost::default()),
        extensions: BTreeMap::new(),
        compaction_tokens: None,
    }
}

fn engine_profile(context: u32, max_out: u32) -> CapabilityProfile {
    CapabilityProfile {
        context_window: context,
        max_output_tokens: max_out,
        tools: true,
        parallel_tools: true,
        vision: true,
        thinking: ThinkingSupport::Adaptive,
        effort: true,
        prompt_caching: true,
        structured_output: false,
        strict_tools: false,
        cost: None,
        extensions: BTreeMap::new(),
        compaction_tokens: None,
    }
}

fn engine_model(id: &str, name: &str, context: u32, max_out: u32) -> ModelDef {
    ModelDef {
        id: id.into(),
        name: name.into(),
        aliases: Vec::new(),
        capabilities: engine_profile(context, max_out),
        family: None,
        default_effort: None,
        tier: None,
    }
}

/// Price a model at the list price of the API model behind it, so a turn
/// carries a cost that reports, replays and budgets can bind to. A provider
/// that publishes no per-token price keeps `None`: a made-up number would
/// corrupt every budget that read it.
fn priced(mut model: ModelDef, cost: ModelCost) -> ModelDef {
    model.capabilities.cost = Some(cost);
    model
}

/// Every current Claude model has a 1M-token window.
const CLAUDE_CONTEXT: u32 = 1_000_000;

/// Codex works from a 272k window (its catalog's `context_window`).
const CODEX_CONTEXT: u32 = 272_000;

/// Models the Codex CLI and the ChatGPT backend accept, by their Codex slugs.
/// Each family's short name is an alias of its newest explicit version.
/// Unpriced: a ChatGPT subscription bills no per-token dollars, so the runtime
/// estimates a turn from the API model behind the slug
/// ([`crate::Registry::api_equivalent_price`]) and reports it as an estimate.
fn codex_models() -> Vec<ModelDef> {
    vec![
        engine_model("gpt-6-astra", "GPT-6 Astra", CODEX_CONTEXT, 128_000)
            .with_aliases(["astra"])
            .with_tier(ModelTier::Strong),
        engine_model("gpt-6.1-sol", "GPT-6.1 Sol", CODEX_CONTEXT, 128_000)
            .with_aliases(["sol"])
            .with_default_effort(GPT_6_1_SOL_EFFORT)
            .with_tier(ModelTier::Balanced),
        engine_model("gpt-6-luna", "GPT-6 Luna", CODEX_CONTEXT, 128_000)
            .with_aliases(["luna"])
            .with_tier(ModelTier::Fast),
        engine_model("default", "Codex default model", CODEX_CONTEXT, 128_000),
    ]
}

/// [`codex_models`] for an operator-supplied endpoint that bills per token:
/// each slug the OpenAI API prices under the same id carries that entry's
/// price, so the two tables cannot drift; any other slug stays unpriced.
fn priced_codex_models() -> Vec<ModelDef> {
    let api = openai_current_models();
    codex_models()
        .into_iter()
        .map(|model| {
            let list_price = api
                .iter()
                .find(|api| api.id == model.id)
                .and_then(|api| api.capabilities.cost);
            match list_price {
                Some(cost) => priced(model, cost),
                None => model,
            }
        })
        .collect()
}

/// Claude subscription through the official Claude Code CLI. Model ids are
/// passed through `--model`; each family's short name is an alias of its newest
/// explicit version, so an older CLI's floating alias cannot select an older
/// model. `default` leaves the choice to the CLI.
fn claude_code() -> ProviderDef {
    ProviderDef {
        id: "claude-code".into(),
        name: "Claude Code (subscription)".into(),
        protocol: Protocol::ClaudeCode,
        base_url: String::new(),
        auth: AuthMethods::cli(crate::auth::cli::CliAuth {
            program: "claude".into(),
            status: vec!["auth".into(), "status".into()],
            login: vec!["auth".into(), "login".into()],
            logout: vec!["auth".into(), "logout".into()],
            logged_in_markers: vec!["\"loggedIn\": true".into(), "\"loggedIn\":true".into()],
            label: "Claude subscription via Claude Code".into(),
            credential_file: Some("~/.claude/.credentials.json".into()),
            credential_dir_env: Some("CLAUDE_CONFIG_DIR".into()),
        }),
        headers: BTreeMap::new(),
        models: vec![
            priced(
                engine_model(
                    "claude-opus-5-5",
                    "Claude Opus 5.5",
                    CLAUDE_CONTEXT,
                    128_000,
                )
                .with_aliases(["opus", "opus-5-5", "opus-5.5"])
                .with_tier(ModelTier::Strong),
                opus_cost(),
            ),
            // No price: `default` is whatever the CLI picks, so pricing it
            // would attribute one model's cost to another.
            engine_model(
                "default",
                "Claude Code default model",
                CLAUDE_CONTEXT,
                64_000,
            ),
            priced(
                engine_model(
                    "claude-fable-5-1",
                    "Claude Fable 5.1",
                    CLAUDE_CONTEXT,
                    128_000,
                )
                .with_aliases(["fable", "fable-5-1"])
                .with_tier(ModelTier::Frontier),
                fable_cost(),
            ),
            priced(
                engine_model(
                    "claude-sonnet-5-5",
                    "Claude Sonnet 5.5",
                    CLAUDE_CONTEXT,
                    128_000,
                )
                .with_aliases(["sonnet", "sonnet-5-5", "sonnet-5.5"])
                .with_tier(ModelTier::Balanced),
                sonnet_cost(),
            ),
            priced(
                engine_model(
                    "claude-haiku-5-5",
                    "Claude Haiku 5.5",
                    CLAUDE_CONTEXT,
                    128_000,
                )
                .with_aliases(["haiku", "haiku-5-5", "haiku-5.5"])
                .with_tier(ModelTier::Fast),
                haiku_cost(),
            ),
        ],
        default_capabilities: Some(engine_profile(CLAUDE_CONTEXT, 64_000)),
        flags: ProviderFlags::default(),
        max_concurrent: None,
        family: Some(ModelFamily::Claude),
        enabled: true,
        usage_probe: Default::default(),
    }
}

/// ChatGPT subscription through the official Codex CLI.
fn codex_cli() -> ProviderDef {
    ProviderDef {
        id: "codex".into(),
        name: "Codex (ChatGPT subscription)".into(),
        protocol: Protocol::Codex,
        base_url: String::new(),
        auth: AuthMethods::cli(crate::auth::cli::CliAuth {
            program: "codex".into(),
            status: vec!["login".into(), "status".into()],
            login: vec!["login".into()],
            logout: vec!["logout".into()],
            logged_in_markers: vec!["Logged in".into()],
            label: "ChatGPT subscription via Codex".into(),
            credential_file: None,
            credential_dir_env: None,
        }),
        headers: BTreeMap::new(),
        models: codex_models(),
        default_capabilities: Some(engine_profile(CODEX_CONTEXT, 128_000)),
        flags: ProviderFlags::default(),
        max_concurrent: None,
        family: Some(ModelFamily::Codex),
        enabled: true,
        usage_probe: Default::default(),
    }
}

pub fn providers() -> Vec<ProviderDef> {
    let mut openrouter = compat(
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        &["OPENROUTER_API_KEY"],
        vec![
            // A router serves several families, so its models say which.
            ModelDef::new(
                "anthropic/claude-sonnet-4.6",
                "Claude Sonnet 4.6 (OpenRouter)",
                openai_like(1_000_000, 64_000, true),
            )
            .with_family(ModelFamily::Claude),
            ModelDef::new(
                "openai/gpt-5",
                "GPT-5 (OpenRouter)",
                openai_like(400_000, 128_000, true),
            )
            .with_family(ModelFamily::Gpt),
        ],
        openai_like(128_000, 16_000, false),
    );
    openrouter.auth = openrouter.auth.with_oauth(OAuthKind::Openrouter);
    openrouter.headers.insert(
        "HTTP-Referer".into(),
        "https://github.com/mengsig/aether".into(),
    );
    openrouter.headers.insert("X-Title".into(), "Aether".into());

    vec![
        claude_code(),
        codex_cli(),
        anthropic(),
        openai(),
        openai_responses(),
        openai_codex(),
        openrouter,
        compat(
            "groq",
            "Groq",
            "https://api.groq.com/openai/v1",
            &["GROQ_API_KEY"],
            vec![
                ModelDef::new(
                    "openai/gpt-oss-120b",
                    "GPT-OSS 120B",
                    openai_like(131_072, 32_768, true),
                ),
                ModelDef::new(
                    "llama-3.3-70b-versatile",
                    "Llama 3.3 70B",
                    openai_like(131_072, 32_768, false),
                ),
            ],
            openai_like(131_072, 8_192, false),
        ),
        compat(
            "xai",
            "xAI",
            "https://api.x.ai/v1",
            &["XAI_API_KEY"],
            vec![
                ModelDef::new("grok-4", "Grok 4", openai_like(256_000, 32_000, false)),
                ModelDef::new(
                    "grok-4-fast",
                    "Grok 4 Fast",
                    openai_like(2_000_000, 32_000, false),
                ),
            ],
            openai_like(256_000, 32_000, false),
        ),
        compat(
            "google",
            "Google Gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            vec![
                ModelDef::new(
                    "gemini-2.5-pro",
                    "Gemini 2.5 Pro",
                    openai_like(1_048_576, 65_536, true),
                ),
                ModelDef::new(
                    "gemini-2.5-flash",
                    "Gemini 2.5 Flash",
                    openai_like(1_048_576, 65_536, true),
                ),
            ],
            openai_like(1_048_576, 65_536, false),
        ),
        compat(
            "ollama",
            "Ollama",
            "http://localhost:11434/v1",
            &[],
            vec![],
            local_profile(),
        ),
        compat(
            "vllm",
            "vLLM",
            "http://localhost:8000/v1",
            &[],
            vec![],
            local_profile(),
        ),
        compat(
            "lmstudio",
            "LM Studio",
            "http://localhost:1234/v1",
            &[],
            vec![],
            local_profile(),
        ),
        compat(
            "llamacpp",
            "llama.cpp",
            "http://localhost:8080/v1",
            &[],
            vec![],
            local_profile(),
        ),
    ]
}

#[cfg(test)]
mod tests;
