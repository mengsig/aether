use super::*;
use aether_runtime::transfer::{export_run, import_run};

#[tokio::test]
async fn feedback_audit_travels_with_remapped_ids_but_does_not_create_memory_records() {
    let (source, spec, agent) = fixture().await;
    show(&source, agent, spec.task, vec![spec.memory], true);
    let feedback = record(&source, &spec);
    source
        .runtime
        .retract_memory_feedback(feedback.id, "Review withdrawn".into(), Actor::user())
        .unwrap();
    source
        .runtime
        .cancel_agent(agent, None, Actor::user())
        .unwrap();
    let run = source.store.agent(agent).unwrap().unwrap().run;
    let path = source.workspace_root.join("feedback.aether");
    export_run(source.store.as_ref(), run, &path).unwrap();
    let target = harness(Vec::new()).await;
    let imported = import_run(&target.runtime, &path).unwrap();
    let events = target
        .store
        .events(0, 1000, &EventFilter::for_run(imported.run))
        .unwrap();
    let feedback_events: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let EventKind::Custom { name, payload } = &event.kind
                && name.starts_with("memory.feedback")
            {
                return Some((event, name, payload));
            }
            None
        })
        .collect();
    assert_eq!(feedback_events.len(), 2);
    let (recorded, name, payload) = feedback_events[0];
    assert_eq!(name, "memory.feedback");
    let remapped_feedback: aether_core::id::MemoryFeedbackId =
        serde_json::from_value(payload["feedback"].clone()).unwrap();
    let remapped_memory: MemoryId = serde_json::from_value(payload["memory"].clone()).unwrap();
    assert_ne!(remapped_feedback, feedback.id);
    assert_ne!(remapped_memory, spec.memory);
    assert_ne!(recorded.task_id, Some(spec.task));
    assert_eq!(
        serde_json::to_value(recorded.task_id).unwrap(),
        payload["task"]
    );
    let (retracted, name, withdrawn) = feedback_events[1];
    assert_eq!(name, "memory.feedback_retracted");
    assert_eq!(withdrawn["feedback"], payload["feedback"]);
    assert_eq!(withdrawn["memory"], payload["memory"]);
    assert_eq!(recorded.task_id, retracted.task_id);
    assert!(target.store.memory(remapped_memory).unwrap().is_none());
    let absent = target
        .store
        .memory_feedback_record(remapped_feedback)
        .unwrap();
    assert_eq!(absent, None);
    source.runtime.shutdown().await;
    target.runtime.shutdown().await;
}
