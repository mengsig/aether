use super::table::{Glyphs, cell, column_width};
use super::{CancellationToken, ProviderHealth, acted, connect_or_start, sane};
use crate::doctor::NotServing;
use aether_protocol::AuthInfo;

pub async fn login(
    provider: Option<String>,
    api_key: bool,
    method: Option<String>,
) -> anyhow::Result<()> {
    use aether_model::LoginMethod;
    use aether_model::auth::interaction::TerminalInteraction;
    let models = aether_model::Models::load(None)?;
    let provider = provider.ok_or_else(|| {
        let known: Vec<&str> = models
            .registry()
            .providers()
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        anyhow::anyhow!(
            "name a provider: aether login <provider>; providers: {}",
            known.join(", ")
        )
    })?;
    prepare_login(&models, &provider, api_key, method.as_deref())?;
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

fn prepare_login(
    models: &aether_model::Models,
    provider: &str,
    api_key: bool,
    method: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::IsTerminal as _;
    let cli_handoff = models
        .registry()
        .provider(provider)
        .and_then(|p| p.auth.cli.as_ref());
    let device_flow = !api_key && matches!(method, Some("device" | "device_code"));
    if cli_handoff.is_none()
        && !device_flow
        && models.registry().provider(provider).is_some()
        && !std::io::stdin().is_terminal()
    {
        anyhow::bail!(
            "`aether login {provider}` needs a terminal to read the credential; \
             run it in a terminal (--api-key selects the API-key prompt)"
        );
    }
    if let Some(cli) = cli_handoff {
        anyhow::ensure!(
            std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            "`aether login {provider}` hands off to `{} {}`, which needs a terminal; run it in a terminal",
            sane(&cli.program),
            sane(&cli.login.join(" "))
        );
        eprintln!(
            "{}: {} · handing off to `{} {}`",
            sane(provider),
            sane(&cli.label),
            sane(&cli.program),
            sane(&cli.login.join(" "))
        );
    }
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
    // Probed as `aether providers` probes, so the two never disagree.
    let providers = client.providers().await?;
    let not_serving = not_serving_among(&rows, &providers).await?;
    let glyphs = Glyphs::new(super::display_ascii(false)?);
    for line in auth_table(&rows, &not_serving, glyphs) {
        println!("{line}");
    }
    Ok(())
}

/// Widest a column grows: a longer value is cut rather than shifting the rest.
const MAX_AUTH_COLUMN: usize = 40;

/// `aether auth` for a person: each column as wide as its longest entry, and
/// under a provider that is not ready, what to do about it.
fn auth_table(rows: &[AuthInfo], not_serving: &[NotServing], glyphs: Glyphs) -> Vec<String> {
    let statuses: Vec<&str> = rows
        .iter()
        .map(|r| auth_status_label(r, not_serving))
        .collect();
    let source = |r: &AuthInfo| r.source.clone().unwrap_or_default();
    let widths = [
        column_width(
            "provider",
            rows.iter().map(|r| r.provider.as_str()),
            MAX_AUTH_COLUMN,
        ),
        column_width(
            "name",
            rows.iter().map(|r| r.provider_name.as_str()),
            MAX_AUTH_COLUMN,
        ),
        column_width("status", statuses.iter().copied(), MAX_AUTH_COLUMN),
        column_width(
            "source",
            rows.iter().filter_map(|r| r.source.as_deref()),
            MAX_AUTH_COLUMN,
        ),
    ];
    let line = |cells: [&str; 4], methods: &str| {
        let mut parts: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(text, width)| cell(text, width, glyphs, false))
            .collect();
        parts.push(sane(methods));
        parts.join(" ")
    };
    let mut out = vec![line(["provider", "name", "status", "source"], "methods")];
    for (r, status) in rows.iter().zip(&statuses) {
        let subscription = if r.subscription {
            " (subscription)"
        } else {
            ""
        };
        let methods = format!("{}{subscription}", r.methods.join(","));
        out.push(line(
            [&r.provider, &r.provider_name, status, &source(r)],
            &methods,
        ));
        if let Some(detail) = auth_detail(r) {
            out.push(format!("  {}", sane(&detail)));
        }
    }
    debug_assert!(out.len() > rows.len());
    out
}

/// `ready` only for a provider with credentials. One that needs none says
/// so, unless its server cannot take a turn: then what `aether providers`
/// says of it.
fn auth_status_label(info: &AuthInfo, not_serving: &[NotServing]) -> &'static str {
    if !info.configured {
        return "not set";
    }
    if info.source.as_deref() != Some(aether_model::auth::NO_AUTH_SOURCE) {
        return "ready";
    }
    match not_serving.iter().find(|s| s.provider == info.provider) {
        Some(server) => server.label(),
        None => "no auth needed",
    }
}

/// Why a provider is not ready, worded as what to do. A sign-in a CLI owns
/// (`claude`, `codex`) is made with `aether login`, unless the CLI is missing.
fn auth_detail(info: &AuthInfo) -> Option<String> {
    let detail = info.detail.as_deref().filter(|_| !info.configured)?;
    let cli_missing = detail.contains("not found") || detail.contains("could not run");
    if info.program.is_none() || cli_missing {
        return Some(detail.to_owned());
    }
    let login = format!("run `aether login {}`", info.provider);
    Some(if detail.contains("login expired") {
        format!("login expired, {login}")
    } else if detail.ends_with("not logged in") {
        format!("not signed in, {login}")
    } else {
        format!("not signed in ({detail}), {login}")
    })
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
            b.until.format("%H:%M:%S UTC")
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
    let not_serving = not_serving_among(&client.auth_status().await?, &rows).await?;
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
            probed_status_label(p, &not_serving),
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

/// The keyless local servers that cannot take a turn, found with the probe
/// `aether doctor` uses, so `auth`, `providers` and the doctor agree.
async fn not_serving_among(
    auth: &[AuthInfo],
    providers: &[aether_protocol::ProviderInfo],
) -> anyhow::Result<Vec<NotServing>> {
    let servers = crate::doctor::local_servers(auth, providers);
    crate::doctor::servers_not_serving(&servers, crate::doctor::PROBE_TIMEOUT).await
}

/// [`aether_protocol::ProviderInfo::status_label`], except that a provider
/// whose server cannot take a turn is not `ready`, however configured.
fn probed_status_label(
    info: &aether_protocol::ProviderInfo,
    not_serving: &[NotServing],
) -> &'static str {
    let label = info.status_label();
    match not_serving.iter().find(|s| s.provider == info.provider) {
        Some(server) if label == "ready" => server.label(),
        _ => label,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        auth_detail, auth_status_label, auth_table, model_cost_label, probed_status_label,
    };
    use aether_core::model::{ModelCost, ModelCostTier};

    #[test]
    fn provider_backoff_times_name_the_utc_zone() {
        let mut health = aether_core::provider::ProviderHealth::idle("openai");
        health.backoff = Some(aether_core::provider::ProviderBackoff {
            reason: "rate limited".into(),
            until: "2026-01-01T12:34:56Z".parse().unwrap(),
        });
        let segments = super::health_segments(&health);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1], "backing off (rate limited) until 12:34:56 UTC");
    }

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
        assert_eq!(auth_detail(&info).as_deref(), info.detail.as_deref());
        let json = serde_json::to_value(&info).expect("JSON");
        assert_eq!(json["detail"], info.detail.as_ref().unwrap().as_str());
        info.configured = true;
        assert!(auth_detail(&info).is_none());
    }

    fn cli_login(detail: &str) -> aether_protocol::AuthInfo {
        aether_protocol::AuthInfo {
            provider: "claude-code".into(),
            provider_name: "Claude Code (subscription)".into(),
            configured: false,
            source: None,
            methods: vec!["cli".into()],
            subscription: true,
            detail: Some(detail.into()),
            program: Some("claude".into()),
        }
    }

    #[test]
    fn a_cli_sign_in_says_how_to_sign_in_through_aether() {
        let hint = |detail: &str| auth_detail(&cli_login(detail)).expect("a reason");
        assert_eq!(
            hint("claude: not logged in"),
            "not signed in, run `aether login claude-code`"
        );
        assert_eq!(
            hint("claude: login expired, run `claude login`"),
            "login expired, run `aether login claude-code`"
        );
        assert_eq!(
            hint("claude: status returned non-zero exit (exit status: 1)"),
            "not signed in (claude: status returned non-zero exit (exit status: 1)), run `aether login claude-code`"
        );
        let missing = "claude not found (No such file or directory (os error 2))";
        assert_eq!(hint(missing), missing, "signing in cannot install the CLI");
    }

    fn keyless(provider: &str) -> aether_protocol::AuthInfo {
        aether_protocol::AuthInfo {
            provider: provider.into(),
            provider_name: provider.into(),
            configured: true,
            source: Some(aether_model::auth::NO_AUTH_SOURCE.into()),
            methods: vec!["none".into()],
            subscription: false,
            detail: None,
            program: None,
        }
    }

    #[test]
    fn auth_calls_a_keyless_provider_what_providers_calls_it() {
        use crate::doctor::{Fault, NotServing};
        let down = [NotServing {
            provider: "vllm".into(),
            address: "localhost:8000".into(),
            fault: Fault::Unreachable,
        }];
        assert_eq!(auth_status_label(&keyless("vllm"), &down), "unreachable");
        assert_eq!(
            auth_status_label(&keyless("ollama"), &down),
            "no auth needed"
        );
        let mut info = provider_info("vllm");
        assert_eq!(
            probed_status_label(&info, &down),
            auth_status_label(&keyless("vllm"), &down)
        );
        let mut keyed = keyless("anthropic");
        keyed.source = Some("ANTHROPIC_API_KEY".into());
        assert_eq!(auth_status_label(&keyed, &down), "ready");
        keyed.configured = false;
        assert_eq!(auth_status_label(&keyed, &down), "not set");
        info.enabled = false;
        assert_eq!(probed_status_label(&info, &down), "off");
    }

    #[test]
    fn the_auth_table_lines_up_however_long_a_name() {
        let mut long = keyless("openai-responses-compatible");
        long.provider_name = "A provider with a long display name".into();
        let rows = [cli_login("claude: not logged in"), long, keyless("vllm")];
        let lines = auth_table(&rows, &[], super::Glyphs::new(false));
        let table: Vec<&String> = lines.iter().filter(|l| !l.starts_with("  ")).collect();
        assert_eq!(table.len(), 4, "{lines:#?}");
        let status_at = table[0].find("status").expect("header");
        for line in &table[1..] {
            let status = &line[status_at..];
            assert!(
                status.starts_with("not set") || status.starts_with("no auth needed"),
                "{lines:#?}"
            );
        }
        assert!(lines.contains(&"  not signed in, run `aether login claude-code`".to_owned()));
    }

    fn provider_info(provider: &str) -> aether_protocol::ProviderInfo {
        aether_protocol::ProviderInfo {
            provider: provider.into(),
            name: provider.into(),
            protocol: "openai-chat".into(),
            family: None,
            base_url: "http://127.0.0.1:8000/v1".into(),
            engine: false,
            models: 0,
            open_model_list: true,
            enabled: true,
            configured: true,
            auth_source: Some("no auth required".into()),
            health: aether_core::provider::ProviderHealth::idle(provider),
            headroom: Default::default(),
        }
    }

    #[test]
    fn a_keyless_provider_nothing_answers_at_is_not_ready() {
        use crate::doctor::{Fault, NotServing};
        let mut info = provider_info("vllm");
        let down = |fault| {
            [NotServing {
                provider: "vllm".into(),
                address: "127.0.0.1:8000".into(),
                fault,
            }]
        };
        assert_eq!(probed_status_label(&info, &[]), "ready");
        assert_eq!(
            probed_status_label(&info, &down(Fault::Unreachable)),
            "unreachable"
        );
        assert_eq!(
            probed_status_label(&info, &down(Fault::NoModels)),
            "no models"
        );
        info.enabled = false;
        assert_eq!(probed_status_label(&info, &down(Fault::Unreachable)), "off");
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
