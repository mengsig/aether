use super::*;

#[tokio::test]
async fn inspection_inputs_preserve_cursors_and_enforce_page_bounds() {
    use crate::builtin::inspection::InspectionTool;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let task = TaskId::new();
    let agent = AgentId::new();
    assert_eq!(
        act(&InspectionTool::Agents, &ctx, json!({})).await.unwrap(),
        HarnessAction::ListAgents {
            after: None,
            limit: 10
        }
    );
    assert_eq!(
        act(
            &InspectionTool::Agents,
            &ctx,
            json!({"after":agent,"limit":25})
        )
        .await
        .unwrap(),
        HarnessAction::ListAgents {
            after: Some(agent),
            limit: 25
        }
    );
    assert_eq!(
        act(&InspectionTool::Tasks, &ctx, json!({"after":task}))
            .await
            .unwrap(),
        HarnessAction::ListTasks {
            after: Some(task),
            limit: 10
        }
    );
    assert_eq!(
        act(&InspectionTool::Task, &ctx, json!({"task":task}))
            .await
            .unwrap(),
        HarnessAction::ReadTask {
            task,
            revision: None,
            offset: 0,
            max_chars: 2000
        }
    );
    assert_eq!(
        act(
            &InspectionTool::Task,
            &ctx,
            json!({"task":task,"revision":"version","offset":256,"max_chars":8192})
        )
        .await
        .unwrap(),
        HarnessAction::ReadTask {
            task,
            revision: Some("version".into()),
            offset: 256,
            max_chars: 8192
        }
    );
    for tool in [InspectionTool::Agents, InspectionTool::Tasks] {
        for input in [
            json!({"limit":0}),
            json!({"limit":26}),
            json!({"limit":-1}),
            json!({"after":"bad"}),
            json!({"unknown":true}),
        ] {
            assert!(act(&tool, &ctx, input).await.is_err());
        }
    }
    for input in [
        json!({}),
        json!({"task":agent}),
        json!({"task":task,"max_chars":255}),
        json!({"task":task,"max_chars":8193}),
        json!({"task":task,"offset":-1}),
        json!({"task":task,"unknown":true}),
    ] {
        assert!(act(&InspectionTool::Task, &ctx, input).await.is_err());
    }
}

#[tokio::test]
async fn cancellation_requires_a_valid_id_and_nonempty_reason() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let agent = AgentId::new();
    assert_eq!(
        act(
            &CancelAgent,
            &ctx,
            json!({"agent":agent,"reason":"obsolete"})
        )
        .await
        .unwrap(),
        HarnessAction::CancelAgent {
            agent,
            reason: "obsolete".into()
        }
    );
    for input in [
        json!({"agent":"not-an-id","reason":"obsolete"}),
        json!({"agent":agent,"reason":" "}),
        json!({"agent":agent}),
        json!({"agent":agent,"reason":"obsolete","include_parent":true}),
    ] {
        assert!(act(&CancelAgent, &ctx, input).await.is_err());
    }
    assert!(!CancelAgent.interruptible());
}

#[tokio::test]
async fn recurring_schedule_inputs_preserve_target_and_timer_policy() {
    use crate::builtin::schedules::ScheduleTool;
    use aether_core::schedule::{
        BusyPolicy, MissedRunPolicy, Recurrence, ScheduleAction, ScheduleTarget,
    };
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let input =
        json!({"name":"Read docs","every_seconds":60,"payload":"Read the docs","max_deliveries":2});
    let HarnessAction::CreateSchedule { spec } = act(&ScheduleTool::Create, &ctx, input.clone())
        .await
        .unwrap()
    else {
        panic!("create action");
    };
    assert_eq!(spec.target, ScheduleTarget::Agent(ctx.agent_id));
    assert_eq!(spec.missed, MissedRunPolicy::Coalesce);
    assert_eq!(spec.busy, BusyPolicy::Skip);
    assert_eq!(spec.max_deliveries.unwrap().get(), 2);
    assert!(matches!(spec.recurrence, Recurrence::Interval { seconds } if seconds.get() == 60));
    for patch in [
        json!({"every_seconds":0}),
        json!({"every_seconds":59}),
        json!({"cron":"* * * * *"}),
        json!({"timezone":"UTC"}),
        json!({"max_deliveries":0}),
        json!({"target":"tsk_wrong"}),
        json!({"busy":"later"}),
    ] {
        let mut invalid = input.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(act(&ScheduleTool::Create, &ctx, invalid).await.is_err());
    }
    let mut queued = input.clone();
    queued["busy"] = json!("queue_one");
    let HarnessAction::CreateSchedule { spec } =
        act(&ScheduleTool::Create, &ctx, queued).await.unwrap()
    else {
        panic!("create action");
    };
    assert_eq!(spec.busy, BusyPolicy::QueueOne);
    let id = aether_core::id::ScheduleId::new();
    assert_eq!(
        act(
            &ScheduleTool::Update,
            &ctx,
            json!({"id":id,"action":"pause"})
        )
        .await
        .unwrap(),
        HarnessAction::UpdateSchedule {
            id,
            action: ScheduleAction::Pause
        }
    );
    assert!(!ScheduleTool::Create.interruptible());
    assert!(!ScheduleTool::Update.interruptible());
    assert!(ScheduleTool::List.interruptible());
}

#[tokio::test]
async fn state_queries_validate_their_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    assert_eq!(
        act(&InspectState, &ctx, json!({})).await.unwrap(),
        HarnessAction::InspectState
    );
    assert!(
        act(&InspectState, &ctx, json!({"agent":"someone_else"}))
            .await
            .is_err()
    );
    assert_eq!(
        act(&ListModels, &ctx, json!({"provider":"codex"}))
            .await
            .unwrap(),
        HarnessAction::ListModels {
            provider: Some("codex".into())
        }
    );
    assert!(
        act(&ListModels, &ctx, json!({"provider":" "}))
            .await
            .is_err()
    );
}
