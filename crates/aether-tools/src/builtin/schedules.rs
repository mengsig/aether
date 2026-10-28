//! Agent-facing recurring timers; execution stays in the runtime.

use std::num::NonZeroU64;

use aether_core::id::{AgentId, ScheduleId};
use aether_core::schedule::{
    BusyPolicy, CreateScheduleSpec, MIN_INTERVAL_SECONDS, MissedRunPolicy, Recurrence,
    ScheduleAction, ScheduleTarget,
};
use aether_core::tool::{ResourceKey, SideEffectClass, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::tool::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, parse_input};

/// Distinguishes an omitted field (`#[serde(default)]` fires, giving `None`)
/// from one present with any value, including `null` (giving `Some`). A
/// plain `Option<Value>` field can't tell these apart - JSON `null` and a
/// missing key both deserialize to `None` - so an explicit `null` on a
/// field with a default would otherwise be silently treated as "use the
/// default" instead of the type error it actually is.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Debug)]
pub enum ScheduleTool {
    Create,
    List,
    Update,
}

#[async_trait]
impl Tool for ScheduleTool {
    fn descriptor(&self) -> ToolDescriptor {
        let (name, description, schema) = match self {
            Self::Create => (
                "create_schedule",
                "Create a durable recurring Aether timer that delivers to an existing agent. Use every_seconds OR a five-field cron expression with an IANA timezone (UTC by default). Target defaults to yourself; only yourself or your descendants are allowed. Payload is the future task instruction. Timers deliver inbox triggers under existing budgets and authority. Paused targets skip occurrences. When the target is busy, busy=skip (default) skips the occurrence; busy=queue_one delivers it while the target runs, once the previous trigger was consumed. Never more than one pending trigger per timer. Use max_deliveries for a finite timer, and list_schedules for history. This is distinct from one-shot wait_for(wake_at). This tool cannot create a spawn schedule (one that spawns a fresh agent per occurrence): that requires operator authority and is only available through the `aether schedule create --objective` CLI.",
                json!({
                    "type":"object", "properties": {
                        "name":{"type":"string"}, "target":{"type":"string"},
                        "every_seconds":{"type":"integer","minimum":MIN_INTERVAL_SECONDS}, "cron":{"type":"string"},
                        "timezone":{"type":"string"}, "payload":{},
                        "missed":{"type":"string","enum":["coalesce","skip"]},
                        "busy":{"type":"string","enum":["skip","queue_one"]},
                        "max_deliveries":{"type":"integer","minimum":1}
                    }, "required":["name","payload"], "additionalProperties":false
                }),
            ),
            Self::List => (
                "list_schedules",
                "List recurring timers you own or that target you. Supply a schedule id to inspect its configuration and latest delivery/skip/failure history. A delivered trigger is not a claim that the requested work finished.",
                json!({"type":"object","properties":{"id":{"type":"string"}},"additionalProperties":false}),
            ),
            Self::Update => (
                "update_schedule",
                "Pause, resume, or cancel a recurring timer you own. Pause/cancel prevents future deliveries; already delivered work is retained. Resume a failed timer only after fixing its cause; it starts with a future occurrence. Cancelled and exhausted timers cannot resume.",
                json!({"type":"object","properties":{"id":{"type":"string"},"action":{"type":"string","enum":["pause","resume","cancel"]}},"required":["id","action"],"additionalProperties":false}),
            ),
        };
        super::descriptor(name, description, schema, SideEffectClass::ReadOnly, false)
    }

    fn interruptible(&self) -> bool {
        matches!(self, Self::List)
    }

    fn resources(&self, _input: &Value, _ctx: &ToolContext) -> Vec<ResourceKey> {
        Vec::new()
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolExecution, ToolError> {
        let action = match self {
            Self::Create => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    name: String,
                    // Raw `Value`s, not typed id/enum fields: direct
                    // deserialization into those types loses the field name
                    // through `serde::de::Error::custom`, and a `String`
                    // field would drop it too for a wrong-typed value.
                    // Parsed below via `parse_field` so a rejection names
                    // the exact `create_schedule.*` field either way.
                    target: Option<Value>,
                    every_seconds: Option<NonZeroU64>,
                    cron: Option<String>,
                    timezone: Option<String>,
                    payload: Value,
                    // `missed`/`busy` use `present` (not a plain
                    // `Option<Value>`) so an explicit `null` is rejected
                    // rather than silently treated the same as an omitted
                    // field defaulting to `MissedRunPolicy`/`BusyPolicy`'s
                    // default.
                    #[serde(default, deserialize_with = "present")]
                    missed: Option<Value>,
                    #[serde(default, deserialize_with = "present")]
                    busy: Option<Value>,
                    max_deliveries: Option<NonZeroU64>,
                }
                let input: Input = parse_input(input)?;
                let target = input
                    .target
                    .map(|v| super::parse_field::<AgentId>(&v, "create_schedule.target"))
                    .transpose()?;
                let missed = input
                    .missed
                    .map(|v| super::parse_field::<MissedRunPolicy>(&v, "create_schedule.missed"))
                    .transpose()?
                    .unwrap_or_default();
                let busy = input
                    .busy
                    .map(|v| super::parse_field::<BusyPolicy>(&v, "create_schedule.busy"))
                    .transpose()?
                    .unwrap_or_default();
                let recurrence = match (input.every_seconds, input.cron, input.timezone) {
                    (Some(seconds), None, None) if seconds.get() < MIN_INTERVAL_SECONDS => {
                        return Err(ToolError::InvalidInput(format!(
                            "every_seconds must be at least {MIN_INTERVAL_SECONDS}; got {seconds}"
                        )));
                    }
                    (Some(seconds), None, None) => Recurrence::Interval { seconds },
                    (None, Some(expression), timezone) => Recurrence::Cron {
                        expression,
                        timezone: timezone.unwrap_or_else(|| "UTC".into()),
                    },
                    _ => {
                        return Err(ToolError::InvalidInput(
                            "choose every_seconds OR cron; timezone is only valid with cron".into(),
                        ));
                    }
                };
                HarnessAction::CreateSchedule {
                    // Only `Agent` delivery targets: `checked_template` accepts a spawn
                    // target only from an operator actor, never a tool call's `Actor::Agent`.
                    spec: CreateScheduleSpec {
                        name: input.name,
                        target: ScheduleTarget::Agent(target.unwrap_or(ctx.agent_id)),
                        recurrence,
                        missed,
                        busy,
                        payload: input.payload,
                        max_deliveries: input.max_deliveries,
                    },
                }
            }
            Self::List => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    id: Option<Value>,
                }
                let input: Input = parse_input(input)?;
                let id = input
                    .id
                    .map(|v| super::parse_field::<ScheduleId>(&v, "list_schedules.id"))
                    .transpose()?;
                HarnessAction::ListSchedules { id }
            }
            Self::Update => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Input {
                    id: Value,
                    action: Value,
                }
                let input: Input = parse_input(input)?;
                let id: ScheduleId = super::parse_field(&input.id, "update_schedule.id")?;
                let action: ScheduleAction =
                    super::parse_field(&input.action, "update_schedule.action")?;
                HarnessAction::UpdateSchedule { id, action }
            }
        };
        Ok(ToolExecution::Action(action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_in;

    async fn act(
        tool: &dyn Tool,
        ctx: &ToolContext,
        input: Value,
    ) -> Result<HarnessAction, ToolError> {
        match tool.execute(input, ctx, CancellationToken::new()).await? {
            ToolExecution::Action(a) => Ok(a),
            ToolExecution::Outcome(_) => Err(ToolError::Internal("unexpected outcome".into())),
        }
    }

    fn invalid_input(err: &ToolError) -> &str {
        match err {
            ToolError::InvalidInput(msg) => msg,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn create_schedule_descriptor_documents_the_spawn_limitation() {
        let description = ScheduleTool::Create.descriptor().spec.description;
        assert!(
            description.contains("cannot create a spawn schedule"),
            "{description}"
        );
        assert!(
            description.contains("aether schedule create --objective"),
            "{description}"
        );
    }

    #[tokio::test]
    async fn create_schedule_target_names_field_value_and_form() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let bad = ScheduleId::new().to_string(); // wrong prefix for an agent id
        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "target": bad}),
        )
        .await
        .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("create_schedule.target"), "{msg}");
        assert!(msg.contains(&bad), "{msg}");
        assert!(msg.contains("agt_"), "{msg}");
    }

    #[tokio::test]
    async fn create_schedule_busy_names_field_value_and_forms() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "busy": "sometimes"}),
        )
        .await
        .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("create_schedule.busy"), "{msg}");
        assert!(msg.contains("sometimes"), "{msg}");
        assert!(msg.contains("skip"), "{msg}");
    }

    #[tokio::test]
    async fn create_schedule_missed_names_field_value_and_forms() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "missed": "retry"}),
        )
        .await
        .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("create_schedule.missed"), "{msg}");
        assert!(msg.contains("retry"), "{msg}");
        assert!(msg.contains("coalesce"), "{msg}");
    }

    #[tokio::test]
    async fn list_schedules_id_names_field_value_and_form() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let bad = AgentId::new().to_string(); // wrong prefix for a schedule id
        let err = act(&ScheduleTool::List, &ctx, json!({"id": bad}))
            .await
            .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("list_schedules.id"), "{msg}");
        assert!(msg.contains(&bad), "{msg}");
        assert!(msg.contains("sch_"), "{msg}");
    }

    /// A wrong-*typed* value (not just a wrong-valued one) must also
    /// name the field - routing these fields through `Value` rather than a
    /// pre-typed `String`/enum struct field is what makes that possible.
    #[tokio::test]
    async fn wrong_json_type_still_names_the_field() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "busy": 5}),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_input(&err).contains("create_schedule.busy"),
            "{err:?}"
        );

        let err = act(
            &ScheduleTool::Update,
            &ctx,
            json!({"id": ScheduleId::new().to_string(), "action": true}),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_input(&err).contains("update_schedule.action"),
            "{err:?}"
        );
    }

    /// For `missed` and `busy` too, an explicit `null` is a wrong-typed
    /// value, not a request for the default, and must be rejected the same as any other bad value.
    #[tokio::test]
    async fn explicit_null_on_a_defaulted_field_is_rejected_not_defaulted() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "missed": null}),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_input(&err).contains("create_schedule.missed"),
            "{err:?}"
        );

        let err = act(
            &ScheduleTool::Create,
            &ctx,
            json!({"name": "n", "payload": {}, "every_seconds": 60, "busy": null}),
        )
        .await
        .unwrap_err();
        assert!(
            invalid_input(&err).contains("create_schedule.busy"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn update_schedule_id_and_action_name_field_value_and_form() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let bad_id = AgentId::new().to_string(); // wrong prefix for a schedule id
        let err = act(
            &ScheduleTool::Update,
            &ctx,
            json!({"id": bad_id, "action": "pause"}),
        )
        .await
        .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("update_schedule.id"), "{msg}");
        assert!(msg.contains(&bad_id), "{msg}");
        assert!(msg.contains("sch_"), "{msg}");

        let id = ScheduleId::new().to_string();
        let err = act(
            &ScheduleTool::Update,
            &ctx,
            json!({"id": id, "action": "delete"}),
        )
        .await
        .unwrap_err();
        let msg = invalid_input(&err);
        assert!(msg.contains("update_schedule.action"), "{msg}");
        assert!(msg.contains("delete"), "{msg}");
        assert!(msg.contains("pause"), "{msg}");
    }

    #[tokio::test]
    async fn valid_calls_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let target = AgentId::new();
        let action = act(
            &ScheduleTool::Create,
            &ctx,
            json!({
                "name": "n", "payload": {}, "every_seconds": 60,
                "target": target.to_string(), "busy": "queue_one", "missed": "skip"
            }),
        )
        .await
        .unwrap();
        match action {
            HarnessAction::CreateSchedule { spec } => {
                assert_eq!(spec.target, ScheduleTarget::Agent(target));
                assert_eq!(spec.busy, BusyPolicy::QueueOne);
                assert_eq!(spec.missed, MissedRunPolicy::Skip);
            }
            other => panic!("{other:?}"),
        }

        let id = ScheduleId::new();
        let action = act(
            &ScheduleTool::Update,
            &ctx,
            json!({"id": id.to_string(), "action": "resume"}),
        )
        .await
        .unwrap();
        assert_eq!(
            action,
            HarnessAction::UpdateSchedule {
                id,
                action: ScheduleAction::Resume
            }
        );
    }

    /// Every advertised `additionalProperties: false` input here must
    /// reject an unknown field rather than silently drop it.
    #[tokio::test]
    async fn unknown_field_is_rejected_on_every_input() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_in(dir.path());
        let cases: Vec<(&dyn Tool, Value)> = vec![
            (
                &ScheduleTool::Create,
                json!({"name": "n", "payload": {}, "every_seconds": 60, "bogus": 1}),
            ),
            (&ScheduleTool::List, json!({"bogus": 1})),
            (
                &ScheduleTool::Update,
                json!({"id": ScheduleId::new().to_string(), "action": "pause", "bogus": 1}),
            ),
        ];
        for (tool, input) in cases {
            let name = tool.descriptor().spec.name;
            let err = act(tool, &ctx, input).await.unwrap_err();
            let msg = invalid_input(&err);
            assert!(msg.contains("bogus"), "{name}: {msg}");
        }
    }
}
