//! Turning `[[mcp.servers]]` configuration into connectable servers, and
//! turning their health transitions into durable events.

use std::sync::Arc;

use aether_core::mcp::{McpAuth, McpAuthState, McpServerHealth};
use aether_model::config::{McpAuthMode, McpSection, McpServerSection};
use aether_runtime::Runtime;
use aether_tools::{McpServerSpec, McpTransportSpec};

/// Build the specs the tool runtime connects with. A token is resolved here,
/// once, from the environment the daemon started with: Aether never stores it.
pub(crate) fn specs(section: &McpSection) -> Vec<McpServerSpec> {
    section.servers.iter().filter_map(spec).collect()
}

fn spec(section: &McpServerSection) -> Option<McpServerSpec> {
    let mut auth = None;
    // Configuration was validated at load; a server that slips through with
    // no transport is skipped loudly rather than connected to nothing.
    let transport = match (&section.command, &section.url) {
        (Some(command), None) => McpTransportSpec::Stdio {
            command: command.into(),
            args: section.args.clone(),
            env: section.env.clone(),
            cwd: section.cwd.clone(),
        },
        (None, Some(url)) => {
            let (token, resolved) = match &section.token_env {
                Some(var) => resolve_token(&section.name, var, section.auth, |name| {
                    std::env::var(name).ok()
                }),
                None => (None, None),
            };
            auth = resolved;
            McpTransportSpec::Http {
                url: url.clone(),
                token,
            }
        }
        _ => {
            tracing::error!(
                server = %section.name,
                "mcp: server names no usable transport; skipping it"
            );
            return None;
        }
    };
    Some(McpServerSpec {
        name: section.name.clone(),
        transport,
        classes: section.classes.clone(),
        send_caller_meta: section.send_caller_meta,
        auth,
    })
}

/// Read a server's token from `var`. A missing or empty one fails auth
/// unless the server is configured `auth = "optional"`: a typo in a variable
/// name must not silently downgrade the server to anonymous access.
fn resolve_token(
    server: &str,
    var: &str,
    mode: McpAuthMode,
    lookup: impl Fn(&str) -> Option<String>,
) -> (Option<String>, Option<McpAuth>) {
    debug_assert!(
        !var.is_empty(),
        "config validation refuses an empty token_env"
    );
    let token = lookup(var).filter(|value| !value.is_empty());
    let state = match (&token, mode) {
        (Some(_), _) => McpAuthState::Token,
        (None, McpAuthMode::Optional) => {
            tracing::info!(
                server = %server,
                variable = %var,
                "mcp: token environment variable is unset; auth is optional, connecting without one"
            );
            McpAuthState::Anonymous
        }
        (None, McpAuthMode::Required) => {
            tracing::warn!(
                server = %server,
                variable = %var,
                "mcp: token environment variable is unset; not connecting (auth_failed)"
            );
            McpAuthState::AuthFailed
        }
    };
    debug_assert_eq!(token.is_some(), state == McpAuthState::Token);
    let auth = McpAuth {
        token_env: var.to_owned(),
        state,
    };
    (token, Some(auth))
}

/// Drain the manager's health channel into the event log for as long as the
/// daemon lives.
///
/// The runtime owns the registry, which owns the manager, which owns the
/// sender: holding a strong reference here would make the runtime undroppable.
/// A weak one ends the task with the runtime instead.
pub(crate) fn spawn_recorder(
    runtime: &Arc<Runtime>,
    mut changes: tokio::sync::mpsc::UnboundedReceiver<McpServerHealth>,
) {
    let runtime = Arc::downgrade(runtime);
    tokio::spawn(async move {
        while let Some(health) = changes.recv().await {
            let Some(runtime) = runtime.upgrade() else {
                break;
            };
            if let Err(e) = runtime.record_mcp_state(&health) {
                tracing::error!(server = %health.name, error = %e, "could not record mcp state");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(name: &str) -> McpServerSection {
        McpServerSection {
            name: name.into(),
            command: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            url: None,
            token_env: None,
            auth: McpAuthMode::Required,
            classes: Default::default(),
            send_caller_meta: false,
        }
    }

    #[test]
    fn a_server_without_a_transport_is_skipped_not_connected_to_nothing() {
        let servers = vec![
            McpServerSection {
                command: Some("mock".into()),
                ..section("good")
            },
            section("broken"),
        ];
        let specs = specs(&McpSection { servers });
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "good");
    }

    fn http(name: &str, token_env: &str, auth: McpAuthMode) -> McpServerSection {
        McpServerSection {
            url: Some("https://notes.example/mcp".into()),
            token_env: Some(token_env.into()),
            auth,
            ..section(name)
        }
    }

    #[test]
    fn a_missing_required_token_fails_auth_instead_of_going_anonymous() {
        let unset = |_: &str| None;
        for lookup in [&unset as &dyn Fn(&str) -> Option<String>, &|_| {
            Some(String::new())
        }] {
            let (token, auth) =
                resolve_token("notes", "NOTES_TOKEN", McpAuthMode::Required, lookup);
            assert_eq!(token, None);
            let auth = auth.expect("a token variable is always reported");
            assert_eq!(auth.state, McpAuthState::AuthFailed);
            assert_eq!(auth.token_env, "NOTES_TOKEN");
        }

        let (token, auth) = resolve_token("notes", "NOTES_TOKEN", McpAuthMode::Optional, unset);
        assert_eq!(token, None);
        assert_eq!(auth.map(|a| a.state), Some(McpAuthState::Anonymous));

        let (token, auth) = resolve_token("notes", "NOTES_TOKEN", McpAuthMode::Required, |var| {
            (var == "NOTES_TOKEN").then(|| "s3cret".to_owned())
        });
        assert_eq!(token.as_deref(), Some("s3cret"));
        assert_eq!(auth.map(|a| a.state), Some(McpAuthState::Token));
    }

    #[test]
    fn the_spec_carries_the_auth_state_and_a_stdio_server_has_none() {
        let var = "AETHER_TEST_MCP_TOKEN_THAT_IS_NEVER_SET";
        assert!(std::env::var_os(var).is_none());
        let servers = vec![
            http("required", var, McpAuthMode::Required),
            http("optional", var, McpAuthMode::Optional),
            McpServerSection {
                command: Some("mock".into()),
                ..section("local")
            },
        ];
        let specs = specs(&McpSection { servers });
        let states: Vec<_> = specs
            .iter()
            .map(|s| s.auth.as_ref().map(|a| a.state))
            .collect();
        assert_eq!(
            states,
            [
                Some(McpAuthState::AuthFailed),
                Some(McpAuthState::Anonymous),
                None
            ]
        );
    }

    /// A config written before `send_caller_meta` existed, for a server that
    /// refuses params it does not know, must keep working with no migration.
    #[tokio::test]
    async fn an_existing_config_keeps_a_strict_server_working() {
        let mock = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/mock_mcp_server.sh"
        );
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.toml"),
            format!(
                "[[mcp.servers]]\nname = \"strict\"\ncommand = \"bash\"\n\
                 args = [{mock:?}]\nenv = {{ AETHER_MOCK_MCP_STRICT = \"1\" }}\n"
            ),
        )
        .expect("write config");
        let config = aether_model::config::load_config(dir.path()).expect("config");
        let specs = specs(&config.mcp);
        assert!(!specs[0].send_caller_meta, "off unless configured");

        let manager = aether_tools::McpManager::new(specs, None, Vec::new().into());
        manager.discover_all().await;
        let caller = aether_tools::McpCaller {
            agent_id: aether_core::id::AgentId::new(),
            task_id: None,
        };
        let result = manager
            .call(
                "strict",
                "read_note",
                serde_json::json!({"key": "k"}),
                &caller,
            )
            .await
            .expect("the strict server is sent nothing it refuses");
        assert_eq!(result.text, "note contents");
    }
}
