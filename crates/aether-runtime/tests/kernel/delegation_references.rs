//! Task references: an approved image reaches the child's model as a picture
//! on its first request and on every later one, and passes on by delegation.
use super::*;
use aether_core::reference::{ReferenceKind, ReferenceRequest};
use aether_model::testing::ScriptedResponse;
use base64::Engine as _;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\napproved mockup";

fn image_count(request: &aether_core::model::ModelRequest) -> usize {
    let png = base64::engine::general_purpose::STANDARD.encode(PNG);
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::Image { data, .. } if *data == png))
        .count()
}

fn system_text(request: &aether_core::model::ModelRequest) -> String {
    request
        .system
        .iter()
        .map(|block| block.text.as_str())
        .collect()
}

struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
    mockup: PathBuf,
}

fn workspace() -> Workspace {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical");
    let mockup = root.join("mock.png");
    std::fs::write(&mockup, PNG).expect("write the mockup");
    std::fs::create_dir(root.join("images")).expect("attachment dir");
    Workspace {
        _dir: dir,
        root,
        mockup,
    }
}

async fn referencing_harness(ws: &Workspace, responses: Vec<ScriptedResponse>) -> Harness {
    let attachments = ws.root.join("images");
    let root = ws.root.clone();
    harness_configured(
        responses,
        |_| {},
        move |config| {
            config.attachment_dir = Some(attachments);
            config.workspace_root = Some(root);
        },
    )
    .await
}

fn with_mockup(mut spec: CreateAgentSpec, ws: &Workspace) -> CreateAgentSpec {
    spec.scope = Some(aether_core::agent::Scope {
        workspace_root: Some(ws.root.clone()),
        ..Default::default()
    });
    spec.references = vec![ReferenceRequest {
        path: ws.mockup.clone(),
        kind: ReferenceKind::Image,
        label: Some("approved mockup".into()),
    }];
    spec
}

#[tokio::test]
async fn a_reference_image_reaches_every_request_and_never_the_transcript() {
    let ws = workspace();
    let h = referencing_harness(
        &ws,
        vec![
            ScriptedClient::text("first"),
            ScriptedClient::text("second"),
        ],
    )
    .await;
    let mut chat = with_mockup(spec("build the settings page"), &ws);
    chat.config.as_mut().expect("config").long_lived = true;
    let (agent, task) = h
        .runtime
        .create_agent(chat, Actor::user())
        .expect("create with a reference");
    let task = task.expect("a task");
    assert_eq!(task.references.len(), 1);
    let stored = task.references[0].image.clone().expect("stored image");
    assert!(ws.root.join("images").join(stored.file_name()).is_file());
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.runtime
        .send_message(
            agent.id,
            vec![ContentBlock::text("and now?")],
            Actor::user(),
        )
        .expect("send");
    h.wait_for("both requests", || h.client.remaining() == 0)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;

    let requests = h.client.requests();
    assert!(requests.len() >= 2, "{}", requests.len());
    for request in &requests {
        assert_eq!(image_count(request), 1, "once on every request");
        let system = system_text(request);
        assert!(system.contains("approved mockup"), "{system}");
        assert!(
            system.contains(&ws.mockup.display().to_string()),
            "{system}"
        );
    }
    let transcript = h.store.transcript(agent.id, 0).expect("transcript");
    assert!(
        !transcript.iter().any(|entry| entry
            .message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ImageRef(_)))),
        "the reference is rebuilt from the task, never recorded as a message"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_missing_reference_refuses_the_creation_and_stores_nothing() {
    let ws = workspace();
    let h = referencing_harness(&ws, Vec::new()).await;
    let mut missing = with_mockup(spec("build"), &ws);
    missing.references[0].path = ws.root.join("gone.png");
    let refused = h
        .runtime
        .create_agent(missing, Actor::user())
        .expect_err("a missing file");
    assert!(refused.to_string().contains("gone.png"), "{refused}");
    let mut no_task = with_mockup(spec("build"), &ws);
    no_task.objective = None;
    let refused = h
        .runtime
        .create_agent(no_task, Actor::user())
        .expect_err("references need a task");
    assert!(refused.to_string().contains("objective"), "{refused}");
    let stored = std::fs::read_dir(ws.root.join("images"))
        .expect("attachment dir")
        .count();
    assert_eq!(stored, 0, "a refusal stores no image");
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_lead_passes_its_references_on_only_from_its_scope_or_by_inheriting() {
    let ws = workspace();
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let outside = elsewhere.path().join("secret.png");
    std::fs::write(&outside, PNG).expect("write");
    let delegate = |extra: serde_json::Value| {
        let mut input = json!({"role": "worker", "objective": "part", "model": "anthropic/claude-sonnet-5-5", "effort": "low", "wait": false, "leaf": true});
        input
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        ScriptedClient::tool_call("delegate", input)
    };
    let h = referencing_harness(
        &ws,
        vec![
            delegate(json!({"references": [{"path": outside, "kind": "image"}]})),
            delegate(json!({"inherit_references": true})),
        ],
    )
    .await;
    let mut lead = with_mockup(spec("lead the change"), &ws);
    lead.role = "lead".into();
    let (parent, lead_task) = h.runtime.create_agent(lead, Actor::user()).expect("lead");
    let lead_task = lead_task.expect("lead task");
    let answers = || {
        h.store
            .events(0, 10_000, &EventFilter::for_agent(parent.id))
            .expect("events")
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::ToolSucceeded { outcome, .. } => Some(outcome.content),
                EventKind::ToolFailed { error, .. } => Some(error),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    h.wait_for("both delegate answers", || answers().len() >= 2)
        .await;
    let answers = answers();
    assert!(answers[0].contains("outside your workspace"), "{answers:?}");
    assert!(answers[1].contains("carries 1 reference(s)"), "{answers:?}");
    let children = h
        .store
        .agents(&AgentFilter {
            parent: Some(parent.id),
            ..Default::default()
        })
        .expect("children");
    assert_eq!(children.len(), 1, "the refused call created no child");
    let task = children[0].current_task.expect("child task");
    let task = h.store.task(task).expect("read").expect("task");
    assert_eq!(task.references, lead_task.references);
    h.runtime.shutdown().await;
}

fn png_of(size: usize, tag: u8) -> aether_core::attachment::ImageAttachment {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.push(tag);
    bytes.resize(size, tag);
    aether_core::attachment::ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        name: None,
    }
}

/// Later message images that alone fill a request's image budget must not
/// crowd out the task's reference: it is charged first and always sent.
async fn assert_the_reference_outlasts(
    messages: Vec<Vec<aether_core::attachment::ImageAttachment>>,
) {
    let ws = workspace();
    let replies = (0..=messages.len())
        .map(|turn| ScriptedClient::text(&format!("turn {turn}")))
        .collect();
    let h = referencing_harness(&ws, replies).await;
    let mut chat = with_mockup(spec("build the settings page"), &ws);
    chat.config.as_mut().expect("config").long_lived = true;
    let (agent, _) = h.runtime.create_agent(chat, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    for (sent, images) in messages.iter().enumerate() {
        let stored = h.runtime.store_attachments(images).expect("store");
        let content = aether_runtime::attachments::message_content("look".into(), stored);
        h.runtime
            .send_message(agent.id, content, Actor::user())
            .expect("send");
        let left = messages.len() - sent - 1;
        h.wait_for("the turn", || h.client.remaining() == left)
            .await;
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
    }
    let requests = h.client.requests();
    let last = requests.last().expect("a request");
    let inline = last
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::Image { .. }))
        .count();
    let png = base64::engine::general_purpose::STANDARD.encode(PNG);
    let leading = last.messages[0]
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { data, .. } if *data == png));
    assert!(
        leading,
        "the leading reference turn still holds the picture"
    );
    assert!(
        inline <= aether_core::attachment::MAX_REQUEST_IMAGES,
        "{inline}"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_reference_image_outlasts_more_message_images_than_a_request_takes() {
    let small = |n: u8| png_of(64, n);
    let batches = (0..3u8)
        .map(|batch| (0..10u8).map(|i| small(batch * 10 + i)).collect())
        .collect();
    assert_the_reference_outlasts(batches).await;
}

#[tokio::test]
async fn a_reference_image_outlasts_message_images_that_fill_the_request_bytes() {
    let big = |n: u8| png_of(aether_core::attachment::MAX_IMAGE_BYTES, n);
    assert_the_reference_outlasts(vec![vec![big(1), big(2)], vec![big(3), big(4)]]).await;
}

#[tokio::test]
async fn a_later_copy_of_the_reference_cannot_take_its_place_in_the_request() {
    let copy = aether_core::attachment::ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(PNG),
        name: Some("copy.png".into()),
    };
    let small = |n: u8| png_of(64, n);
    let mut batches = vec![vec![copy]];
    batches.extend((0..2u8).map(|batch| (0..10u8).map(|i| small(batch * 10 + i)).collect()));
    assert_the_reference_outlasts(batches).await;
}

/// Words a user sends cannot pose as the reference turn: with no task
/// references, a message carrying its text is ordinary history and the newest
/// images still win the budget (the unit test in `attachments` covers a first
/// message that is exactly the intro).
#[tokio::test]
async fn a_message_echoing_the_reference_intro_gets_no_place_ahead_of_newer_images() {
    let ws = workspace();
    let replies = (0..4)
        .map(|turn| ScriptedClient::text(&format!("turn {turn}")))
        .collect();
    let h = referencing_harness(&ws, replies).await;
    let mut chat = spec("build the settings page");
    chat.config.as_mut().expect("config").long_lived = true;
    let (agent, _) = h.runtime.create_agent(chat, Actor::user()).expect("create");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let intro = aether_context::references::REFERENCE_INTRO.to_owned();
    let mimic = aether_core::attachment::ImageAttachment {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(PNG),
        name: None,
    };
    let mut batches = vec![(intro, vec![mimic])];
    for batch in 0..2u8 {
        let images = (0..10u8).map(|i| png_of(64, batch * 10 + i)).collect();
        batches.push(("look".to_owned(), images));
    }
    for (sent, (text, images)) in batches.iter().enumerate() {
        let stored = h.runtime.store_attachments(images).expect("store");
        let content = aether_runtime::attachments::message_content(text.clone(), stored);
        h.runtime
            .send_message(agent.id, content, Actor::user())
            .expect("send");
        let left = 4 - sent - 2;
        h.wait_for("the turn", || h.client.remaining() == left)
            .await;
        h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
            .await;
    }
    let requests = h.client.requests();
    let last = requests.last().expect("a request");
    assert_eq!(
        image_count(last),
        0,
        "the oldest image gave way to newer ones"
    );
    let inline = last
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::Image { .. }))
        .count();
    assert_eq!(inline, aether_core::attachment::MAX_REQUEST_IMAGES);
    h.runtime.shutdown().await;
}
