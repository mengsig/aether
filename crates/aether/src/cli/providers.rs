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
    println!(
        "{:<18} {:<24} {:<7} {:<8} {:<20} {:>6}  health",
        "provider", "protocol", "family", "status", "auth", "models"
    );
    for p in &rows {
        println!(
            "{:<18} {:<24} {:<7} {:<8} {:<20} {:>6}  {}",
            sane(&p.provider),
            sane(&p.protocol),
            sane(p.family.as_deref().unwrap_or("-")),
            p.status_label(),
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

#[cfg(test)]
mod tests {
    use super::{auth_detail, model_cost_label};
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
}
