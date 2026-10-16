use super::*;
use aether_core::id::{AgentId, RunId, TaskId};
use std::time::Duration;

fn context() -> EngineLaunchContext {
    EngineLaunchContext {
        run_id: RunId::from_ulid(ulid::Ulid::from(0x123456789abcdef_u128)),
        agent_id: AgentId::from_ulid(ulid::Ulid::from(2_u128)),
        task_id: Some(TaskId::from_ulid(ulid::Ulid::from(3_u128))),
    }
}

#[tokio::test]
async fn spawned_telemetry_environment() {
    for case in [
        "enabled",
        "empty",
        "absent",
        "disabled",
        "hidden",
        "hidden-enable",
        "taskless",
        "legacy",
        "zero",
    ] {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "claude_code::tests::telemetry::child_environment_probe",
                "--nocapture",
            ])
            .env("AETHER_TELEMETRY_TEST_CASE", case)
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "team=example,aether.agent_id=previous",
            )
            .env("TRACEPARENT", "user-parent")
            .env_remove("CLAUDE_CODE_ENABLE_TELEMETRY")
            .kill_on_drop(true);
        if case != "disabled" {
            command.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1");
        }
        if case == "empty" {
            command.env("OTEL_RESOURCE_ATTRIBUTES", "");
        }
        if case == "absent" {
            command.env_remove("OTEL_RESOURCE_ATTRIBUTES");
        }
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[tokio::test]
async fn child_environment_probe() {
    let Ok(case) = std::env::var("AETHER_TELEMETRY_TEST_CASE") else {
        return;
    };
    let inherited = std::env::var("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default();
    let mut identity = context();
    if case == "taskless" {
        identity.task_id = None;
    }
    if case == "zero" {
        identity.run_id = RunId::from_ulid(ulid::Ulid::from(0_u128));
    }
    for summary_only in [false, true] {
        let dir = fake_claude("read -r line\nenv > child-env\n");
        let engine = ClaudeCodeEngine::new(dir.path().join("claude").to_string_lossy(), "2.1.265");
        let mut turn = turn();
        turn.cwd = dir.path().into();
        turn.summary_only = summary_only;
        if case == "hidden" {
            turn.hidden_env = vec!["OTEL_RESOURCE_ATTRIBUTES".into(), "TRACEPARENT".into()];
        }
        if case == "hidden-enable" {
            turn.hidden_env.push("CLAUDE_CODE_ENABLE_TELEMETRY".into());
        }
        let mut run = if case == "legacy" {
            engine.run(turn, CancellationToken::new()).await.unwrap()
        } else {
            engine
                .run_with_context(turn, identity, CancellationToken::new())
                .await
                .unwrap()
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while run.events.recv().await.is_some() {}
        })
        .await
        .unwrap();
        let environment = std::fs::read_to_string(dir.path().join("child-env")).unwrap();
        check_environment(&case, &environment, &inherited, identity);
    }
    assert_eq!(
        std::env::var("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default(),
        inherited
    );
    assert_eq!(std::env::var("TRACEPARENT").unwrap(), "user-parent");
}

fn check_environment(case: &str, env: &str, inherited: &str, context: EngineLaunchContext) {
    let value = |key: &str| {
        env.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
    };
    let attributes = value("OTEL_RESOURCE_ATTRIBUTES");
    let parent = value("TRACEPARENT");
    if case == "hidden" {
        assert_eq!(attributes, None);
        assert_eq!(parent, None);
        return;
    }
    if matches!(case, "disabled" | "legacy" | "hidden-enable") {
        assert_eq!(attributes, Some(inherited));
        assert_eq!(parent, Some("user-parent"));
        return;
    }
    let prefix = if inherited.is_empty() {
        String::new()
    } else {
        format!("{inherited},")
    };
    let task = context
        .task_id
        .map(|id| format!(",aether.task_id={id}"))
        .unwrap_or_default();
    assert_eq!(
        attributes.unwrap(),
        format!(
            "{prefix}aether.run_id={},aether.agent_id={}{task}",
            context.run_id, context.agent_id
        )
    );
    check_traceparent(case, parent.unwrap());
}

fn check_traceparent(case: &str, parent: &str) {
    assert_eq!(parent.len(), 55);
    let fields: Vec<_> = parent.split('-').collect();
    assert_eq!(fields[0], "00");
    assert_eq!(fields[3], "01");
    assert_eq!(fields[1].len(), 32);
    assert_eq!(fields[2].len(), 16);
    assert!(fields[1..3].iter().all(|s| {
        s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }));
    assert_ne!(u128::from_str_radix(fields[1], 16).unwrap(), 0);
    assert_ne!(u64::from_str_radix(fields[2], 16).unwrap(), 0);
    assert_eq!(
        parent,
        if case == "zero" {
            "00-00000000000000000000000000000001-0000000000000001-01"
        } else {
            "00-00000000000000000123456789abcdef-0123456789abcdef-01"
        }
    );
}
