use std::collections::BTreeMap;
use std::sync::Arc;

use aether_core::agent::Scope;
use aether_core::artifact::{Artifact, ArtifactKind};
use aether_core::id::{AgentId, TaskId};
use aether_model::auth::AuthMethods;
use aether_model::registry::{Protocol, ProviderOverride, ProvidersFile, Registry};
use aether_tools::{ArtifactSink, ToolContext, ToolError};

use super::tool_registry;

const KEY: &str = "AETHER_TEST_PROVIDER_KEY";
const TOKEN: &str = "AETHER_TEST_PROVIDER_TOKEN";
const PUBLIC: &str = "AETHER_TEST_PUBLIC_ENV";

fn providers() -> Registry {
    let mut registry = Registry::builtin();
    registry
        .apply_overrides(ProvidersFile {
            providers: BTreeMap::from([(
                "custom".into(),
                ProviderOverride {
                    protocol: Some(Protocol::OpenaiChatCompletions),
                    base_url: Some("https://example.com".into()),
                    auth: Some(AuthMethods::api_key([KEY]).with_bearer_env([TOKEN])),
                    ..Default::default()
                },
            )]),
        })
        .expect("custom provider");
    registry
}

#[test]
fn provider_credentials_are_hidden_from_tools_but_retained_for_engines() {
    let config = aether_model::config::AetherConfig::default();
    let tools = tool_registry(&config, &providers(), None);
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        KEY,
        TOKEN,
    ] {
        assert!(
            tools.hidden_env().contains(&name.to_owned()),
            "{name} reaches tools"
        );
        assert!(
            !tools.engine_hidden_env().contains(&name.to_owned()),
            "{name} stripped from engines"
        );
    }
    for name in aether_tools::registry::DAEMON_ENV
        .into_iter()
        .chain([config.listen.token_env.as_str()])
    {
        assert!(
            tools.engine_hidden_env().contains(&name.to_owned()),
            "{name} reaches engines"
        );
    }
}

struct UnusedArtifacts;

#[async_trait::async_trait]
impl ArtifactSink for UnusedArtifacts {
    async fn store(
        &self,
        _kind: ArtifactKind,
        _name: &str,
        _summary: &str,
        _content_type: Option<&str>,
        _bytes: Vec<u8>,
        _agent: AgentId,
        _task: Option<TaskId>,
    ) -> Result<Artifact, ToolError> {
        Err(ToolError::Internal("unexpected artifact".into()))
    }
}

#[tokio::test]
async fn a_host_shell_hides_inherited_provider_keys_and_keeps_public_env() {
    // Re-exec isolates the daemon environment without mutating a running test process.
    if std::env::var(PUBLIC).as_deref() != Ok("visible") {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "env_tests::a_host_shell_hides_inherited_provider_keys_and_keeps_public_env",
                "--nocapture",
            ])
            .env(KEY, "secret")
            .env(TOKEN, "secret")
            .env(PUBLIC, "visible")
            .output()
            .expect("isolated test process");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    assert_eq!(std::env::var(KEY).as_deref(), Ok("secret"));
    let dir = tempfile::tempdir().expect("workspace");
    let tools = tool_registry(&Default::default(), &providers(), None);
    let ctx = ToolContext {
        agent_id: AgentId::new(),
        task_id: None,
        instance_id: "test".into(),
        scope: Scope::default(),
        cwd: dir.path().into(),
        artifacts: Arc::new(UnusedArtifacts),
        env: BTreeMap::new(),
        hidden_env: tools.hidden_env(),
        temp_dir: None,
        build_env: Vec::new(),
    };
    let command = format!(
        "printf '%s|%s|%s' \"${{{KEY}-absent}}\" \"${{{TOKEN}-absent}}\" \"${{{PUBLIC}-absent}}\""
    );
    let output = aether_tools::exec::prepare(&command, &ctx)
        .expect("prepare")
        .command
        .output()
        .await
        .expect("shell");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("text"),
        "absent|absent|visible"
    );
}
