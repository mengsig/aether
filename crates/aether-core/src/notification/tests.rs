use super::*;
use crate::time::now;

fn post(title: &str) -> NotificationPost {
    NotificationPost {
        kind: NotificationKind::Question,
        title: title.to_owned(),
        body: "Item **qi_1** needs a merge decision.".into(),
        key: Some("q-7".into()),
        link: None,
        actions: vec![
            NotificationAction {
                id: "yes".into(),
                label: "Merge".into(),
            },
            NotificationAction {
                id: "no".into(),
                label: "Hold".into(),
            },
        ],
        free_text: false,
    }
}

fn posted() -> Notification {
    Notification::new("archon".into(), post("Merge #12?"), now())
}

#[test]
fn a_checked_post_becomes_an_unread_notification() {
    let n = posted();
    assert_eq!(n.state, NotificationState::Unread);
    assert_eq!(n.key.as_deref(), Some("q-7"));
    assert!(n.is_answerable());
    assert_eq!(n.poster_agent(), None);
    let agent = AgentId::new();
    let mine = Notification::new(agent.to_string(), post("x"), now());
    assert_eq!(mine.poster_agent(), Some(agent));
}

#[test]
fn a_post_breaking_a_limit_is_refused_with_the_limit_named() {
    let mut multi_line = post("one\ntwo");
    assert!(multi_line.check().unwrap_err().contains("one line"));
    multi_line.title = "x".repeat(MAX_TITLE_CHARS + 1);
    assert!(multi_line.check().unwrap_err().contains("200"));
    let mut body = post("t");
    body.body = "b".repeat(MAX_BODY_BYTES + 1);
    assert!(body.check().unwrap_err().contains("4096"));
    let mut twice = post("t");
    twice.actions[1].id = "yes".into();
    assert!(twice.check().unwrap_err().contains("used twice"));
    let mut blank_key = post("t");
    blank_key.key = Some("  ".into());
    assert!(blank_key.check().unwrap_err().contains("key"));
    assert!(post("t").check().is_ok());
}

#[test]
fn service_sources_are_names_and_never_agent_ids() {
    assert!(check_service_source("archon").is_ok());
    assert!(check_service_source("ci.bot-2").is_ok());
    assert!(check_service_source("").is_err());
    assert!(check_service_source("Archon").is_err());
    assert!(check_service_source(&AgentId::new().to_string().to_lowercase()).is_err());
}

#[test]
fn a_repost_changes_content_and_reopens_only_a_read_notification() {
    let mut n = posted();
    assert!(!n.repost(post("Merge #12?"), now()), "an identical repost");
    assert!(n.mark_read(now()));
    assert!(n.repost(post("Merge #12 now?"), now()));
    assert_eq!(n.state, NotificationState::Unread);
    assert!(n.resolve(now()));
    assert!(n.repost(post("Merge #12 later?"), now()));
    assert_eq!(n.state, NotificationState::Resolved);
    assert_eq!(n.title, "Merge #12 later?");
}

#[test]
fn read_and_resolve_change_only_once() {
    let mut n = posted();
    assert!(n.mark_read(now()));
    assert!(!n.mark_read(now()));
    assert!(n.resolve(now()));
    assert!(!n.resolve(now()));
    assert!(
        !n.mark_read(now()),
        "a resolved notification is not read again"
    );
}

#[test]
fn an_answer_must_be_offered_and_resolves_once() {
    let mut n = posted();
    let by = Actor::user();
    let unknown = n.answer(Some("maybe".into()), None, by.clone(), now());
    assert!(unknown.unwrap_err().contains("yes, no"));
    let text = n.answer(None, Some("ship it".into()), by.clone(), now());
    assert!(text.unwrap_err().contains("not text"));
    assert!(
        n.answer(None, Some("  ".into()), by.clone(), now())
            .is_err()
    );
    n.answer(Some("yes".into()), None, by.clone(), now())
        .unwrap();
    assert_eq!(n.state, NotificationState::Resolved);
    assert_eq!(
        n.answer.as_ref().and_then(|a| a.action.as_deref()),
        Some("yes")
    );
    assert!(!n.is_answerable());
    let again = n.answer(Some("no".into()), None, by, now());
    assert!(again.unwrap_err().contains("already answered"));
}

#[test]
fn free_text_is_accepted_only_where_offered() {
    let mut open = post("Which branch?");
    open.actions.clear();
    open.free_text = true;
    let mut n = Notification::new("archon".into(), open, now());
    n.answer(None, Some("next".into()), Actor::user(), now())
        .unwrap();
    assert_eq!(n.answer.unwrap().text.as_deref(), Some("next"));
    let mut info = post("FYI");
    info.actions.clear();
    let info = Notification::new("archon".into(), info, now());
    assert!(!info.is_answerable());
}

#[test]
fn every_change_reads_back_from_its_event() {
    let n = posted();
    for change in NotificationChange::ALL {
        let draft = n.draft(change, Actor::user(), None).unwrap();
        assert_eq!(Notification::of(&draft.kind), Some((change, n.clone())));
        assert_eq!(
            draft.agent_id, None,
            "a service notification is daemon-wide"
        );
    }
    let agent = AgentId::new();
    let run = RunId::new();
    let mine = Notification::new(agent.to_string(), post("x"), now());
    let draft = mine
        .draft(NotificationChange::Posted, Actor::agent(agent), Some(run))
        .unwrap();
    assert_eq!((draft.agent_id, draft.run), (Some(agent), Some(run)));
    let other = EventKind::Custom {
        name: "note.appended".into(),
        payload: serde_json::json!({"text": "hi"}),
    };
    assert_eq!(Notification::of(&other), None);
}

#[test]
fn the_wire_form_leaves_out_empty_optionals() {
    let mut bare = post("t");
    bare.actions.clear();
    bare.key = None;
    bare.body.clear();
    let wire = serde_json::to_value(Notification::new("archon".into(), bare, now())).unwrap();
    for absent in ["body", "key", "link", "actions", "free_text", "answer"] {
        assert!(wire.get(absent).is_none(), "{absent}: {wire}");
    }
    assert_eq!(wire["state"], "unread");
    assert_eq!(wire["kind"], "question");
}
