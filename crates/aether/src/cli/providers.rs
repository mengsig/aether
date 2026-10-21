use super::{CancellationToken, ProviderHealth, acted, connect_or_start, sane};

pub async fn login(provider: String, api_key: bool, method: Option<String>) -> anyhow::Result<()> {
    use aether_model::LoginMethod;
    use aether_model::auth::interaction::TerminalInteraction;
    use std::io::IsTerminal as _;
    let models = aether_model::Models::load(None)?;
    let cli_handoff = models
        .registry()
        .provider(&provider)
        .and_then(|p| p.auth.cli.clone());
    let device_flow = !api_key && matches!(method.as_deref(), Some("device" | "device_code"));
    if cli_handoff.is_none()
        && !device_flow
        && models.registry().provider(&provider).is_some()
        && !std::io::stdin().is_terminal()
    {
        anyhow::bail!(
            "`aether login {provider}` needs a terminal to read the credential; \
             run it in a terminal (--api-key selects the API-key prompt)"
        );
    }
    if let Some(cli) = cli_handoff {
        eprintln!(
            "{}: {} · handing off to `{} {}`",
            sane(&provider),
            sane(&cli.label),
            sane(&cli.program),
            sane(&cli.login.join(" "))
        );
    }
    let has_oauth = models.oauth_label(&provider).is_some();
    let login_method = if api_key || !has_oauth {
        LoginMethod::ApiKey
    } else {
        LoginMethod::OAuth { method }
    };
    if let Some(label) = models.oauth_label(&provider).filter(|_| !api_key) {
        eprintln!("{}: {}", sane(&provider), sane(&label));
    }
    let interaction = TerminalInteraction { open_browser: true };
    let cancel = CancellationToken::new();
    let info = models
        .login(&provider, login_method, &interaction, &cancel)
        .await?;
    println!(
        "stored {:?} credential for {}",
        info.kind,
        sane(&info.provider)
    );
    Ok(())
}

pub async fn logout(provider: String, json: bool) -> anyhow::Result<()> {
    let models = aether_model::Models::load(None)?;
    if models.registry().provider(&provider).is_none() {
        let known: Vec<&str> = models
            .registry()
            .providers()
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        anyhow::bail!(
            "unknown provider `{provider}`; valid providers: {}",
            known.join(", ")
        );
    }
    let stored = models.credential_store().read(&provider).await?.is_some();
    models.logout(&provider).await?;
    let line = if stored {
        format!("removed credentials for {provider}")
    } else {
        format!("no stored credentials for {provider}; nothing to remove")
    };
    acted(
        json,
        &line,
        serde_json::json!({"provider": provider, "removed": stored}),
    )
}

pub async fn auth(socket: impl Into<aether_client::Endpoint>, json: bool) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect_or_start(&socket).await?;
    let rows = client.auth_status().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<16} {:<22} {:<12} {:<28} methods",
        "provider", "name", "status", "source"
    );
    for r in rows {
        println!(
            "{:<16} {:<22} {:<12} {:<28} {}{}",
            sane(&r.provider),
            sane(&r.provider_name),
            if r.configured { "ready" } else { "not set" },
            sane(r.source.as_deref().unwrap_or_default()),
            sane(&r.methods.join(",")),
            if r.subscription {
                " (subscription)"
            } else {
                ""
            }
        );
        if let Some(detail) = auth_detail(&r) {
            println!("  {}", sane(detail));
        }
    }
    Ok(())
}

fn auth_detail(info: &aether_protocol::AuthInfo) -> Option<&str> {
    info.detail.as_deref().filter(|_| !info.configured)
}

pub async fn models(socket: impl Into<aether_client::Endpoint>, json: bool) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect_or_start(&socket).await?;
    let rows = client.list_models().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<40} {:<10} {:>9} {:>8} {:<8} cost in/out per Mtok",
        "model", "status", "context", "output", "think"
    );
    for m in rows {
        let c = &m.capabilities;
        println!(
            "{:<40} {:<10} {:>9} {:>8} {:<8} {}{}",
            sane(&m.model.to_string()),
            if m.configured { "ready" } else { "not set" },
            c.context_window,
            c.max_output_tokens,
            format!("{:?}", c.thinking).to_lowercase(),
            model_cost_label(c.cost),
            if m.is_default { "  (default)" } else { "" }
        );
    }
    Ok(())
}

/// What a run or session on `model` costs, for the line that names it. An
/// engine's own `default` carries no price, so it says so instead of "?".
pub(super) fn model_price_phrase(
    rows: &[aether_protocol::ModelInfo],
    model: &aether_core::model::ModelRef,
) -> String {
    let row = rows.iter().find(|row| {
        row.model.provider == model.provider
            && (row.model.model == model.model || row.aliases.contains(&model.model))
    });
    match row.and_then(|row| row.capabilities.cost) {
        Some(cost) => format!("{} per Mtok in/out", model_cost_label(Some(cost))),
        None => "no list price known".into(),
    }
}

fn model_cost_label(cost: Option<aether_core::model::ModelCost>) -> String {
    let Some(cost) = cost else {
        return "?".into();
    };
    let mut label = format!("${}/${}", cost.input_per_mtok, cost.output_per_mtok);
    if let Some(tier) = cost.above_threshold {
        let threshold = if tier.prompt_tokens > 0 && tier.prompt_tokens.is_multiple_of(1_000) {
            format!("{}K", tier.prompt_tokens / 1_000)
        } else {
            tier.prompt_tokens.to_string()
        };
        label.push_str(&format!(
            " (>{threshold}: ${}/${})",
            tier.input_per_mtok, tier.output_per_mtok
        ));
    }
    label
}

/// One provider's live picture, segment by segment: load first, then the
/// reported window, the backoff, what it has been failing with and where a
/// turn last went instead. Shared so `status` and `providers` cannot drift.
pub(super) fn health_segments(h: &ProviderHealth) -> Vec<String> {
    let mut parts = vec![match (h.limit, h.running) {
        (Some(limit), running) => format!("{running}/{limit}"),
        (None, 0) => "idle".to_owned(),
        (None, running) => running.to_string(),
    }];
    if let Some(q) = &h.quota {
        parts.push(format!(
            "{}% of {}",
            q.used_percent,
            q.window_label().unwrap_or_else(|| "window".into())
        ));
    }
    if let Some(b) = &h.backoff {
        parts.push(format!(
            "backing off ({}) until {}",
            b.reason,
            b.until.format("%H:%M:%S")
        ));
    }
    if let Some(errors) = h.error_summary() {
        parts.push(format!("errors {errors}"));
    }
    if let Some(f) = &h.last_fallback {
        parts.push(format!("last fallback to {} ({})", f.to, f.reason));
    }
    parts.into_iter().map(|part| sane(&part)).collect()
}

pub async fn providers(
    socket: impl Into<aether_client::Endpoint>,
    json: bool,
) -> anyhow::Result<()> {
    let socket = socket.into();
    let client = connect_or_start(&socket).await?;
    let rows = client.providers().await?;
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    let unreachable = unreachable_local_servers(&client, &rows).await?;
    println!(
        "{:<18} {:<24} {:<7} {:<11} {:<20} {:>6}  health",
        "provider", "protocol", "family", "status", "auth", "models"
    );
    for p in &rows {
        println!(
            "{:<18} {:<24} {:<7} {:<11} {:<20} {:>6}  {}",
            sane(&p.provider),
            sane(&p.protocol),
            sane(p.family.as_deref().unwrap_or("-")),
            probed_status_label(p, &unreachable),
            sane(p.auth_source.as_deref().unwrap_or("-")),
            p.model_count(),
            health_segments(&p.health).join(" · ")
        );
        if let Some(usage) = &p.health.usage {
            for window in &usage.windows {
                println!("  {}", sane(&window.quota.headroom_label(&window.name)));
            }
        }
    }
    Ok(())
}

/// The keyless local servers among `rows` that nothing answers at, found
/// with the probe `aether doctor` uses so the two commands agree.
async fn unreachable_local_servers(
    client: &aether_client::Client,
    rows: &[aether_protocol::ProviderInfo],
) -> anyhow::Result<Vec<crate::doctor::Unreachable>> {
    let auth = client.auth_status().await?;
    let servers = crate::doctor::local_servers(&auth, rows);
    Ok(crate::doctor::unreachable_servers(&servers, crate::doctor::PROBE_TIMEOUT).await)
}

/// [`aether_protocol::ProviderInfo::status_label`], except that a provider
/// whose server refused the probe is not `ready`, however configured.
fn probed_status_label(
    info: &aether_protocol::ProviderInfo,
    unreachable: &[crate::doctor::Unreachable],
) -> &'static str {
    let label = info.status_label();
    if label == "ready" && unreachable.iter().any(|u| u.provider == info.provider) {
        "unreachable"
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::{auth_detail, model_cost_label, probed_status_label};
    use aether_core::model::{ModelCost, ModelCostTier};

    #[test]
    fn model_cost_labels_show_long_prompt_rates_and_exact_thresholds() {
        let mut cost = ModelCost {
            input_per_mtok: 0.1,
            output_per_mtok: 0.5,
            ..Default::default()
        };
        assert_eq!(model_cost_label(None), "?");
        assert_eq!(model_cost_label(Some(cost)), "$0.1/$0.5");
        cost.above_threshold = Some(ModelCostTier {
            prompt_tokens: 100_000,
            input_per_mtok: 0.5,
            output_per_mtok: 2.5,
            cache_read_per_mtok: 0.05,
            cache_write_per_mtok: 0.625,
            cache_write_1h_per_mtok: None,
        });
        assert_eq!(model_cost_label(Some(cost)), "$0.1/$0.5 (>100K: $0.5/$2.5)");
        cost.above_threshold.as_mut().unwrap().prompt_tokens = 100_001;
        assert_eq!(
            model_cost_label(Some(cost)),
            "$0.1/$0.5 (>100001: $0.5/$2.5)"
        );
    }
    #[test]
    fn auth_detail_keeps_failure_reason_in_table_and_json() {
        let mut info = aether_protocol::AuthInfo {
            provider: "anthropic".into(),
            provider_name: "Anthropic".into(),
            configured: false,
            source: None,
            methods: vec!["api_key".into()],
            subscription: false,
            detail: Some("credential store: unsafe directory; run chmod 700 config".into()),
            program: None,
        };
        assert_eq!(auth_detail(&info), info.detail.as_deref());
        let json = serde_json::to_value(&info).expect("JSON");
        assert_eq!(json["detail"], info.detail.as_ref().unwrap().as_str());
        info.configured = true;
        assert!(auth_detail(&info).is_none());
    }

    #[test]
    fn a_keyless_provider_nothing_answers_at_is_not_ready() {
        use crate::doctor::Unreachable;
        let mut info = aether_protocol::ProviderInfo {
            provider: "vllm".into(),
            name: "vLLM".into(),
            protocol: "openai-chat".into(),
            family: None,
            base_url: "http://127.0.0.1:8000/v1".into(),
            engine: false,
            models: 0,
            open_model_list: true,
            enabled: true,
            configured: true,
            auth_source: Some("no auth required".into()),
            health: aether_core::provider::ProviderHealth::idle("vllm"),
            headroom: Default::default(),
        };
        let down = [Unreachable {
            provider: "vllm".into(),
            address: "127.0.0.1:8000".into(),
        }];
        assert_eq!(probed_status_label(&info, &[]), "ready");
        assert_eq!(probed_status_label(&info, &down), "unreachable");
        info.enabled = false;
        assert_eq!(probed_status_label(&info, &down), "off");
    }
}

#[cfg(test)]
mod price_tests {
    use super::*;
    use aether_core::model::ModelRef;

    fn row(model: &str, cost: Option<aether_core::model::ModelCost>) -> aether_protocol::ModelInfo {
        let capabilities = aether_core::model::CapabilityProfile {
            cost,
            ..Default::default()
        };
        aether_protocol::ModelInfo {
            tier: None,
            model: ModelRef::new("claude-code", model),
            aliases: vec!["opus".into()],
            name: model.into(),
            provider_name: "Claude Code".into(),
            capabilities,
            configured: true,
            auth_source: None,
            is_default: false,
        }
    }

    #[test]
    fn price_phrase_names_the_rate_or_says_unpriced() {
        let priced = aether_core::model::ModelCost {
            input_per_mtok: 5.0,
            output_per_mtok: 25.0,
            ..Default::default()
        };
        let rows = vec![row("claude-opus-5-5", Some(priced)), row("default", None)];
        let opus = model_price_phrase(&rows, &ModelRef::new("claude-code", "opus"));
        assert!(opus.contains("$5/$25"), "{opus}");
        let default = model_price_phrase(&rows, &ModelRef::new("claude-code", "default"));
        assert!(default.contains("no list price"), "{default}");
        let missing = model_price_phrase(&rows, &ModelRef::new("openai", "gpt-unpriced"));
        assert!(missing.contains("no list price"), "{missing}");
        assert!(!missing.contains("subscription"), "{missing}");
    }
}
