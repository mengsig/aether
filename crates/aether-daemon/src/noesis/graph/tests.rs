use std::time::{Duration, Instant};

use aether_core::artifact::ArtifactKind;
use aether_core::event::{Actor, EventKind};
use aether_core::id::ArtifactId;
use aether_core::noesis::{Job, JobKind, JobState, SessionExtraction};
use aether_noesis::extract::jobs as job_rows;
use aether_noesis::graph::store::{self as graph_store, SourceRow};
use aether_noesis::store::NoesisStore;
use aether_protocol::noesis::{
    BudgetPatch, ConcurrencyPatch, ExtractionPatch, NoesisSettingsPatch,
};
use aether_protocol::{Command, ErrorCode, ErrorReply, Reply};
use aether_runtime::CreateAgentSpec;
use aether_runtime::artifacts::ArtifactPublication;

use super::super::State;
use super::super::tests::Harness;

async fn running(settings: NoesisSettingsPatch) -> Harness {
    let harness = Harness::new().await;
    let status = harness
        .configure(NoesisSettingsPatch {
            enabled: Some(true),
            ..settings
        })
        .await;
    assert!(status.effective.enabled, "{status:?}");
    harness
}

fn background_spent() -> NoesisSettingsPatch {
    NoesisSettingsPatch {
        budget: Some(BudgetPatch {
            background_usd_daily: Some(0.0),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `noesis.db` opened beside the service, as a worker opens it.
fn side_store(harness: &Harness) -> NoesisStore {
    NoesisStore::open(&harness.data.path().join("noesis.db")).expect("noesis.db")
}

fn seed_source(store: &NoesisStore, id: &str) {
    let row = SourceRow {
        id: id.into(),
        name: "Notes".into(),
        artifact: ArtifactId::new().to_string(),
        imported_at: graph_store::now_text(),
    };
    graph_store::insert_source(store.connection(), &row).expect("source");
}

fn seed_session(store: &NoesisStore, id: &str, mode: &str, agent: &str) {
    store
        .connection()
        .execute(
            "INSERT INTO sessions (id, goal, mode, phase, agent, created_at, updated_at)
             VALUES (?1, 'Learn limits', ?2, 'teach', ?3, '2026-11-04T00:00:00Z', '2026-11-04T00:00:00Z')",
            [id, mode, agent],
        )
        .expect("session");
}

async fn publish(harness: &Harness, text: &str) -> ArtifactId {
    let runtime = &harness.daemon.runtime;
    let spec = CreateAgentSpec {
        role: "worker".into(),
        objective: Some("hold an artifact".into()),
        ..Default::default()
    };
    let (agent, task) = runtime.create_agent(spec, Actor::user()).expect("agent");
    let publication = ArtifactPublication {
        kind: ArtifactKind::File,
        name: "notes.md".into(),
        summary: "notes".into(),
        content_type: Some("text/markdown".into()),
        bytes: text.as_bytes().to_vec(),
        agent: agent.id,
        task: task.map(|t| t.id),
    };
    let artifact = runtime.publish_artifact(publication, Actor::user()).await;
    artifact.expect("artifact").id
}

async fn jobs(harness: &Harness) -> Vec<Job> {
    match harness.send(Command::NoesisJobs { state: None }).await {
        Ok(Reply::NoesisJobs { jobs }) => jobs,
        other => panic!("a job list, got {other:?}"),
    }
}

/// The job once it has ended, waiting for its worker.
async fn ended(harness: &Harness, id: &str) -> Job {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found = jobs(harness).await.into_iter().find(|job| job.id == id);
        let job = found.expect("the job is listed");
        if job.state.is_terminal() {
            return job;
        }
        assert!(Instant::now() < deadline, "the job never ended: {job:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn job_events(harness: &Harness) -> Vec<Job> {
    let store = harness.daemon.runtime.store();
    let events = store
        .events(0, 10_000, &aether_store::EventFilter::default())
        .expect("events");
    events
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::NoesisJobUpdated { job } => Some(job),
            _ => None,
        })
        .collect()
}

fn command(json: &str) -> Command {
    serde_json::from_str(json).expect(json)
}

fn code(result: Result<Reply, ErrorReply>) -> ErrorCode {
    result.expect_err("refused").code
}

#[tokio::test]
async fn a_spent_background_budget_fails_an_import_s_job_before_any_model_call() {
    let harness = running(background_spent()).await;
    let artifact = publish(&harness, "Alpha holds.\n\nBeta follows from alpha.").await;
    let import = command(&format!(
        r#"{{"cmd":"noesis_import_source","name":"Notes","artifact":"{artifact}","idempotency_key":"i1"}}"#
    ));
    let first = harness.send(import.clone()).await.expect("import");
    let Reply::NoesisImport { source, job } = first.clone() else {
        panic!("an import reply, got {first:?}");
    };
    assert!(source.starts_with("s_"), "{source}");
    assert_eq!(
        (job.kind, job.target.as_str()),
        (JobKind::ExtractSource, source.as_str())
    );
    let again = harness.send(import).await.expect("retry");
    assert_eq!(again, first, "a keyed retry replays the first import");
    let job = ended(&harness, &job.id).await;
    assert_eq!(job.state, JobState::Failed, "{job:?}");
    let error = job.error.unwrap_or_default();
    assert!(error.contains("refused before the model call"), "{error}");
    let agents = harness
        .daemon
        .runtime
        .store()
        .agents(&Default::default())
        .expect("agents");
    assert!(
        agents
            .iter()
            .all(|agent| !agent.role.0.starts_with("noesis.")),
        "no extraction agent was created"
    );
    let states: Vec<JobState> = job_events(&harness).iter().map(|j| j.state).collect();
    assert_eq!(states.first(), Some(&JobState::Queued), "{states:?}");
    assert_eq!(states.last(), Some(&JobState::Failed), "{states:?}");
    let graph = harness
        .send(command(r#"{"cmd":"noesis_graph"}"#))
        .await
        .expect("graph");
    assert!(
        matches!(graph, Reply::NoesisGraph { ref nodes, .. } if nodes.is_empty()),
        "{graph:?}"
    );
}

/// A token the default redactor masks; `read_artifact` never shows it.
const SOURCE_TOKEN: &str = "ghp_Q7rT2vX9kL4mN8pZ3wY6bC1dF5gH0jKs";

/// The extraction task's objective once its worker has created it.
async fn extraction_objective(harness: &Harness) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let store = harness.daemon.runtime.store();
        let tasks = store.tasks(&Default::default()).expect("tasks");
        if let Some(task) = tasks
            .iter()
            .find(|task| task.objective.contains("Alpha holds"))
        {
            return task.objective.clone();
        }
        assert!(Instant::now() < deadline, "no extraction task was created");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn an_imported_source_reaches_its_extractor_masked_as_read_artifact_shows_it() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let text = format!("Alpha holds.\n\nThe deploy token is {SOURCE_TOKEN}.\n\nBeta follows.");
    let artifact = publish(&harness, &text).await;
    let import =
        format!(r#"{{"cmd":"noesis_import_source","name":"Notes","artifact":"{artifact}"}}"#);
    let Reply::NoesisImport { job, .. } = harness.send(command(&import)).await.expect("import")
    else {
        panic!("an import reply");
    };
    let objective = extraction_objective(&harness).await;
    assert!(
        !objective.contains(SOURCE_TOKEN),
        "the task objective carries the raw token"
    );
    assert!(objective.contains("[REDACTED:github_token]"), "{objective}");
    let store = harness.daemon.runtime.store();
    let events = store
        .events(0, 10_000, &aether_store::EventFilter::default())
        .expect("events");
    let leaked = events.iter().any(|event| {
        serde_json::to_string(&event.kind)
            .expect("event json")
            .contains(SOURCE_TOKEN)
    });
    assert!(!leaked, "an event carries the raw token");
    let cancel = format!(r#"{{"cmd":"noesis_cancel_job","job":"{}"}}"#, job.id);
    harness.send(command(&cancel)).await.expect("cancel");
}

#[tokio::test]
async fn graph_writes_name_what_does_not_exist() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let missing = [
        r#"{"cmd":"noesis_delete_proposition","proposition":"p_missing"}"#,
        r#"{"cmd":"noesis_delete_source","source":"s_missing","graph":"preserve"}"#,
        r#"{"cmd":"noesis_extract","session":"ns_missing"}"#,
        r#"{"cmd":"noesis_extract","source":"s_missing"}"#,
        r#"{"cmd":"noesis_cancel_job","job":"j_missing"}"#,
    ];
    for json in missing {
        assert_eq!(
            code(harness.send(command(json)).await),
            ErrorCode::NotFound,
            "{json}"
        );
    }
    let artifact = ArtifactId::new();
    let import =
        format!(r#"{{"cmd":"noesis_import_source","name":"Notes","artifact":"{artifact}"}}"#);
    assert_eq!(
        code(harness.send(command(&import)).await),
        ErrorCode::NotFound
    );
    let store = side_store(&harness);
    seed_session(&store, "ns_practice", "practice", "agt_x");
    let practice = r#"{"cmd":"noesis_extract","session":"ns_practice"}"#;
    assert_eq!(
        code(harness.send(command(practice)).await),
        ErrorCode::InvalidArgument
    );
    assert!(jobs(&harness).await.is_empty(), "a refusal queues nothing");
}

#[tokio::test]
async fn a_live_job_holds_its_source_until_it_is_cancelled() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let store = side_store(&harness);
    seed_source(&store, "s_notes");
    let queued =
        job_rows::create(store.connection(), JobKind::ExtractSource, "s_notes").expect("job");
    let delete =
        r#"{"cmd":"noesis_delete_source","source":"s_notes","graph":"delete_unsupported"}"#;
    assert_eq!(
        code(harness.send(command(delete)).await),
        ErrorCode::InvalidTransition
    );
    let cancel = format!(r#"{{"cmd":"noesis_cancel_job","job":"{}"}}"#, queued.id);
    let Reply::NoesisJob { job } = harness.send(command(&cancel)).await.expect("cancel") else {
        panic!("a job reply");
    };
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(
        job_events(&harness).last().map(|j| j.state),
        Some(JobState::Cancelled)
    );
    assert_eq!(
        harness.send(command(delete)).await.expect("delete"),
        Reply::Ok
    );
    assert_eq!(
        code(harness.send(command(delete)).await),
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn with_no_background_slot_no_job_is_made() {
    let settings = NoesisSettingsPatch {
        concurrency: Some(ConcurrencyPatch {
            background: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let harness = running(settings).await;
    let store = side_store(&harness);
    seed_source(&store, "s_notes");
    seed_source(&store, "s_other");
    let extract = r#"{"cmd":"noesis_extract","source":"s_notes"}"#;
    let error = harness.send(command(extract)).await.expect_err("no slot");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    assert!(
        error.message.contains("concurrency.background"),
        "{error:?}"
    );
    assert!(jobs(&harness).await.is_empty());
    let queued =
        job_rows::create(store.connection(), JobKind::ExtractSource, "s_other").expect("job");
    let again = r#"{"cmd":"noesis_extract","source":"s_other"}"#;
    let Reply::NoesisJob { job } = harness.send(command(again)).await.expect("extract") else {
        panic!("a job reply");
    };
    assert_eq!(job.id, queued.id, "a live job is returned, not doubled");
    assert_eq!(jobs(&harness).await.len(), 1);
}

#[tokio::test]
async fn a_blank_import_name_is_refused_before_anything_is_stored() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let artifact = publish(&harness, "Alpha holds.").await;
    let import =
        format!(r#"{{"cmd":"noesis_import_source","name":"   ","artifact":"{artifact}"}}"#);
    let error = harness.send(command(&import)).await.expect_err("blank");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    assert!(jobs(&harness).await.is_empty());
    let graph = side_store(&harness);
    let sources: u32 = graph
        .connection()
        .query_row("SELECT count(*) FROM sources", [], |row| row.get(0))
        .expect("count");
    assert_eq!(sources, 0, "no source is recorded");
}

#[tokio::test]
async fn cancelling_a_job_a_worker_claimed_asks_the_worker_and_keeps_its_state() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let store = side_store(&harness);
    seed_source(&store, "s_notes");
    let queued =
        job_rows::create(store.connection(), JobKind::ExtractSource, "s_notes").expect("job");
    let slots = aether_noesis::extract::Slots::new(1);
    let (_slot, claimed) = aether_noesis::extract::claim_next(store.connection(), &slots)
        .expect("claim")
        .expect("a queued job");
    assert_eq!(
        (claimed.id.as_str(), claimed.state),
        (queued.id.as_str(), JobState::Running)
    );
    let cancel = format!(r#"{{"cmd":"noesis_cancel_job","job":"{}"}}"#, queued.id);
    let Reply::NoesisJob { job } = harness.send(command(&cancel)).await.expect("cancel") else {
        panic!("a job reply");
    };
    assert_eq!(
        job.state,
        JobState::Running,
        "a claimed job is not overwritten"
    );
    let state = harness.daemon.noesis.state.lock().await;
    let State::Running(live) = &*state else {
        panic!("the service runs");
    };
    assert!(
        live.extraction.is_cancelling(&queued.id),
        "the worker is asked to stop"
    );
}

#[tokio::test]
async fn sessions_are_queued_after_a_teaching_turn_only_in_auto_mode() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let runtime = &harness.daemon.runtime;
    let service = &harness.daemon.noesis;
    let manual = service.after_teaching_turn(runtime, "ns_missing").await;
    assert_eq!(
        manual.expect("manual"),
        None,
        "manual mode reads and queues nothing"
    );
    let auto = NoesisSettingsPatch {
        extraction: Some(ExtractionPatch {
            sessions: Some(SessionExtraction::Auto),
            ..Default::default()
        }),
        ..Default::default()
    };
    harness.configure(auto).await;
    let error = service.after_teaching_turn(runtime, "ns_missing").await;
    assert!(error.is_err(), "auto mode reads the session: {error:?}");
    let spec = CreateAgentSpec {
        role: "worker".into(),
        ..Default::default()
    };
    let (agent, _) = runtime.create_agent(spec, Actor::user()).expect("agent");
    seed_session(
        &side_store(&harness),
        "ns_quiet",
        "teach",
        &agent.id.to_string(),
    );
    let quiet = service.after_teaching_turn(runtime, "ns_quiet").await;
    assert_eq!(
        quiet.expect("auto"),
        None,
        "below the threshold nothing is queued"
    );
    assert!(jobs(&harness).await.is_empty());
}

#[tokio::test]
async fn a_restarted_service_ends_the_jobs_the_stopped_one_left() {
    let harness = running(NoesisSettingsPatch::default()).await;
    let store = side_store(&harness);
    seed_source(&store, "s_notes");
    let left =
        job_rows::create(store.connection(), JobKind::ExtractSource, "s_notes").expect("job");
    drop(store);
    let restart = NoesisSettingsPatch {
        budget: Some(BudgetPatch {
            interactive_usd_daily: Some(2.0),
            ..Default::default()
        }),
        ..Default::default()
    };
    harness.configure(restart).await;
    {
        let state = harness.daemon.noesis.state.lock().await;
        assert!(
            matches!(&*state, State::Running(_)),
            "the settings change restarted it"
        );
    }
    let job = jobs(&harness)
        .await
        .into_iter()
        .find(|job| job.id == left.id);
    let job = job.expect("listed");
    assert_eq!(job.state, JobState::Failed);
    assert!(job.error.unwrap_or_default().contains("interrupted"));
}
