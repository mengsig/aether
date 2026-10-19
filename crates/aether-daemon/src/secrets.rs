//! The redactor seeded with the secret values this daemon knows, so tool and
//! engine output that echoes one verbatim is masked before it is recorded.
//! The values are read once, compiled into the redactor, and never logged or stored.

use aether_core::redact::Redactor;
use aether_model::config::AetherConfig;
use aether_model::registry::Registry;

use crate::listen::TcpListen;

/// Build the redactor from the listener's token and the values of every
/// variable the daemon treats as a secret: `[listen]`, `[serve]` and MCP
/// `token_env`s, and each provider's API key and bearer variables. aetherd
/// reads its configuration only at start, so a restart is what refreshes them.
pub(crate) fn known_secrets_redactor(
    config: &AetherConfig,
    providers: &Registry,
    tcp: Option<&TcpListen>,
    lookup: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Redactor> {
    let mut names = super::daemon_secret_env(config, tcp);
    names.extend(super::provider_secret_env(providers));
    names.sort_unstable();
    names.dedup();
    debug_assert!(names.iter().all(|name| !name.is_empty()));
    let mut values: Vec<String> = names.iter().filter_map(|name| lookup(name)).collect();
    values.extend(tcp.map(|listen| listen.token.clone()));
    let redactor = Redactor::with_known_secrets(values.iter().map(String::as_str))?;
    Ok(redactor)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aether_model::config::McpServerSection;

    use super::*;

    const LISTEN: &str = "listen-token-0123456789abcdef";
    const MCP: &str = "mcp-notes-token-0123456789";
    const PROVIDER: &str = "provider-key-0123456789abcdef";

    fn config() -> AetherConfig {
        let mut config = AetherConfig::default();
        config.listen.token_env = "FLEET_TOKEN".into();
        config.mcp.servers.push(McpServerSection {
            name: "notes".into(),
            url: Some("https://notes.example/mcp".into()),
            token_env: Some("NOTES_TOKEN".into()),
            command: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            auth: Default::default(),
            classes: Default::default(),
            send_caller_meta: false,
        });
        config
    }

    fn env() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("FLEET_TOKEN", LISTEN),
            ("NOTES_TOKEN", MCP),
            ("OPENAI_API_KEY", PROVIDER),
            ("ANTHROPIC_API_KEY", "short"),
            ("UNRELATED", "unrelated-value-0123456789"),
        ])
    }

    #[test]
    fn configured_listen_mcp_and_provider_values_are_masked() {
        let env = env();
        let redactor = known_secrets_redactor(&config(), &Registry::builtin(), None, |name| {
            env.get(name).map(|value| value.to_string())
        })
        .expect("redactor");
        let echoed = format!("{LISTEN} {MCP} {PROVIDER} short unrelated-value-0123456789");
        let out = redactor.redact(&echoed);
        for secret in [LISTEN, MCP, PROVIDER] {
            assert!(!out.contains(secret), "{secret} survived: {out}");
        }
        assert_eq!(out.matches("[REDACTED:known_secret]").count(), 3, "{out}");
        assert!(
            out.ends_with(" short unrelated-value-0123456789"),
            "only secret variables are masked: {out}"
        );
        assert!(!format!("{redactor:?}").contains(LISTEN));
    }

    #[test]
    fn an_embedder_resolved_listener_token_is_masked() {
        let tcp = TcpListen::new(([127, 0, 0, 1], 0).into(), "embedder-token-0123456789");
        let redactor = known_secrets_redactor(
            &AetherConfig::default(),
            &Registry::builtin(),
            Some(&tcp),
            |_| None,
        )
        .expect("redactor");
        let out = redactor.redact("hello embedder-token-0123456789");
        assert_eq!(out, "hello [REDACTED:known_secret]");
    }
}
