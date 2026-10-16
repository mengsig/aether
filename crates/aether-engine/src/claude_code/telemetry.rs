//! Opt-in correlation for the child process; never mutate the daemon environment.
use std::ffi::OsString;

use crate::EngineLaunchContext;

pub(super) fn environment(
    context: Option<EngineLaunchContext>,
    hidden: &[String],
) -> Vec<(OsString, OsString)> {
    let visible = |name: &str| !hidden.iter().any(|entry| entry == name);
    let Some(context) = context.filter(|_| {
        visible("CLAUDE_CODE_ENABLE_TELEMETRY")
            && std::env::var_os("CLAUDE_CODE_ENABLE_TELEMETRY").is_some()
    }) else {
        return Vec::new();
    };
    let mut env = Vec::with_capacity(2);
    if visible("OTEL_RESOURCE_ATTRIBUTES") {
        let mut attributes = std::env::var_os("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default();
        if !attributes.is_empty() {
            attributes.push(",");
        }
        attributes.push(format!(
            "aether.run_id={},aether.agent_id={}",
            context.run_id, context.agent_id
        ));
        if let Some(task) = context.task_id {
            attributes.push(format!(",aether.task_id={task}"));
        }
        env.push(("OTEL_RESOURCE_ATTRIBUTES".into(), attributes));
    }
    if visible("TRACEPARENT") {
        env.push(("TRACEPARENT".into(), traceparent(context).into()));
    }
    debug_assert!(env.len() <= 2);
    debug_assert!(
        env.iter()
            .all(|(name, _)| !hidden.iter().any(|h| name == h.as_str()))
    );
    env
}

fn traceparent(context: EngineLaunchContext) -> String {
    let run = u128::from(context.run_id.ulid());
    let trace = run.max(1);
    let parent = ((run >> 64) as u64 ^ run as u64).max(1);
    debug_assert_ne!(trace, 0);
    debug_assert_ne!(parent, 0);
    format!("00-{trace:032x}-{parent:016x}-01")
}
