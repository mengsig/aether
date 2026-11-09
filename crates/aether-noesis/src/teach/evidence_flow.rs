//! F6 end to end over the scripted engine: a bundle's status labels reach the
//! Prober and, through the hand-off, the Teacher; either switch off sends
//! nothing; and nothing on the way opens a socket, to archond or anywhere.

use std::collections::BTreeSet;

use aether_core::noesis::{
    EvidenceBundle, EvidenceItem, EvidenceRef, EvidenceRefKind, EvidenceStatus, JobKind, JobState,
};
use serde_json::json;

use super::faux::FauxEngine;
use super::records::*;
use super::testkit::*;
use super::*;
use crate::budget::{Accountant, LaneSpend};
use crate::extract::{
    self, AccountedBudget, CancelFlag, Corpus, ExtractModel, JobPorts, Message, ModelError, Reply,
    Role, Slots,
};
use crate::prompts::Agent;
use crate::settings::{EngineeringExtraction, NoesisSettings};

fn engineering_on() -> NoesisSettings {
    NoesisSettings {
        enabled: true,
        engineering_context: true,
        extraction_engineering: EngineeringExtraction::Auto,
        ..NoesisSettings::default()
    }
}

fn entry(
    kind: EvidenceRefKind,
    id: &str,
    url: &str,
    excerpt: &str,
    status: EvidenceStatus,
) -> EvidenceItem {
    let url = Some(url.to_string());
    EvidenceItem {
        reference: EvidenceRef {
            kind,
            id: id.into(),
            url,
        },
        excerpt: excerpt.into(),
        status,
    }
}

/// Every reference points at `archond`, the guard's address: following one
/// would be a connection the guard sees.
fn bundle(archond: &str) -> EvidenceBundle {
    let at = |path: &str| format!("http://{archond}/archon/{path}");
    EvidenceBundle {
        title: "Ship the cache".into(),
        items: vec![
            entry(
                EvidenceRefKind::Item,
                "it_1",
                &at("items/it_1"),
                "The cache is keyed by the tree hash.",
                EvidenceStatus::Verified,
            ),
            entry(
                EvidenceRefKind::Stage,
                "review",
                &at("items/it_1/stages/review"),
                "A reviewer passed the eviction rule.",
                EvidenceStatus::Reviewed,
            ),
            entry(
                EvidenceRefKind::Agent,
                "agt_7",
                &at("agents/agt_7"),
                "The agent guesses the TTL is five minutes.",
                EvidenceStatus::Preliminary,
            ),
        ],
    }
}

/// Each label beside its excerpt, as the rendered bundle states them.
fn assert_labelled(text: &str, bundle: &EvidenceBundle) {
    for item in &bundle.items {
        let status = crate::evidence::status_name(item.status);
        let excerpt = text
            .find(&item.excerpt)
            .unwrap_or_else(|| panic!("{} is shown", item.excerpt));
        let before = &text[..excerpt];
        let label = before
            .rfind("\"status\": ")
            .expect("a status precedes the excerpt");
        assert!(
            before[label..].starts_with(&format!("\"status\": \"{status}\"")),
            "{status}"
        );
    }
}

fn input_text(input: &Input) -> &str {
    match input {
        Input::Learner(text)
        | Input::Opening(text)
        | Input::Handoff(text)
        | Input::Guidance(text)
        | Input::Failure(text)
        | Input::AcceptedText(text) => text,
        other => panic!("a text input, got {other:?}"),
    }
}

/// Guards run one at a time: no other test of this crate opens a socket.
static GUARDED: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A listener standing in for archond, and the sockets this process held before.
struct SocketGuard {
    listener: std::net::TcpListener,
    before: BTreeSet<String>,
    _alone: std::sync::MutexGuard<'static, ()>,
}

impl SocketGuard {
    fn new() -> Self {
        let alone = GUARDED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            listener,
            before: sockets(),
            _alone: alone,
        }
    }

    fn address(&self) -> String {
        self.listener.local_addr().unwrap().to_string()
    }

    fn assert_untouched(&self) {
        let accepted = self.listener.accept();
        assert!(
            matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "nothing connected to the archond stand-in: {accepted:?}"
        );
        let opened: Vec<_> = sockets().difference(&self.before).cloned().collect();
        assert!(opened.is_empty(), "Noesis opened sockets: {opened:?}");
    }
}

/// The process's open socket descriptors (Linux); empty elsewhere.
fn sockets() -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return BTreeSet::new();
    };
    entries
        .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
        .map(|target| target.to_string_lossy().into_owned())
        .filter(|target| target.starts_with("socket:"))
        .collect()
}

struct NoCorpus;

impl Corpus for NoCorpus {
    fn session_prose(&self, _: &str) -> Result<Vec<Message>, String> {
        Err("an engineering job reads no transcript".into())
    }
    fn source_text(&self, _: &str) -> Result<String, String> {
        Err("an engineering job reads no source".into())
    }
}

/// Records each prompt and states one finding anchored on the first item.
#[derive(Default)]
struct Extractor(Vec<String>);

impl ExtractModel for Extractor {
    fn invoke(&mut self, role: Role, prompt: &str) -> Result<Reply, ModelError> {
        assert_eq!(role, Role::Extractor, "an empty graph needs no audit");
        self.0.push(prompt.to_string());
        let finding = json!({"key": "k0", "name": "Cache key", "anchor": "evidence:0",
                             "statement": "The cache is keyed by the tree hash."});
        let text = json!({"propositions": [finding], "chains": []}).to_string();
        Ok(Reply {
            text,
            usage: Default::default(),
        })
    }
}

fn run_engineering_job(noesis: &crate::Noesis, session: &str) -> (Vec<String>, JobState) {
    let conn = noesis.store().connection();
    let effective = noesis.effective();
    let queued = extract::engineering::after_bundle_stored(conn, &effective, session).unwrap();
    assert_eq!(
        queued.expect("auto queues the job").kind,
        JobKind::ExtractEngineering
    );
    let (_slot, job) = extract::claim_next(conn, &Slots::new(1))
        .unwrap()
        .expect("claimed");
    let mut model = Extractor::default();
    let today = chrono::Utc::now().date_naive();
    let mut budget = AccountedBudget::new(Accountant::new(&effective, today, LaneSpend::default()));
    let ports = JobPorts {
        model: &mut model,
        budget: &mut budget,
        corpus: &NoCorpus,
        cancel: &CancelFlag::default(),
        events: &mut |_| {},
        engineering_in_effect: crate::evidence::in_effect(&effective),
    };
    let ended = extract::run_job(conn, job, ports).unwrap();
    (model.0, ended.state)
}

#[test]
fn status_labels_reach_the_teacher_and_nothing_connects_to_archond() {
    let guard = SocketGuard::new();
    let (_dir, noesis) = start(engineering_on());
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let bundle = bundle(&guard.address());
    let id = t
        .create_session_from("Understand the cache", Mode::Teach, &[], Some(&bundle))
        .unwrap()
        .id;
    let prober = session(&t, &id).agent;
    assert_labelled(input_text(t.engine().inputs_of(&prober)[0]), &bundle);

    t.engine_mut()
        .script(Agent::Prober, "", vec![(PROBE_DONE, strands())]);
    t.engine_mut()
        .script(Agent::Teacher, "The plan.", vec![(PLAN, three_steps())]);
    assert_eq!(t.advance(&id).unwrap()[0].end, TurnEnd::HandedOff);
    let teacher = session(&t, &id).agent;
    assert_ne!(teacher, prober);
    assert_eq!(kind_of(&t, &teacher), Agent::Teacher);
    let handoff = input_text(t.engine().inputs_of(&teacher)[0]).to_string();
    assert!(handoff.contains("The Prober's assessment"));
    assert_labelled(&handoff, &bundle);

    let (prompts, state) = run_engineering_job(&noesis, &id);
    assert_eq!(state, JobState::Succeeded);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("[verified] item it_1")
            && prompts[0].contains("[reviewed] stage review")
    );
    assert!(
        !prompts[0].contains("five minutes"),
        "a preliminary item never enters the graph"
    );
    let graph = crate::graph::store::load(noesis.store().connection()).unwrap();
    let origins: Vec<_> = graph
        .nodes
        .values()
        .flat_map(|n| n.provenance.iter())
        .collect();
    assert_eq!(origins.len(), 1);
    assert_eq!(
        (origins[0].source.as_str(), origins[0].message_ref.as_str()),
        (id.as_str(), "evidence:0")
    );
    guard.assert_untouched();
}

#[test]
fn with_either_switch_off_a_bundle_is_refused_before_anything_is_sent() {
    let guard = SocketGuard::new();
    let bundle = bundle(&guard.address());
    let off = [
        NoesisSettings {
            engineering_context: false,
            ..engineering_on()
        },
        NoesisSettings::default(),
    ];
    for settings in off {
        let (_dir, noesis) = start(settings);
        let mut t = Teaching::new(&noesis, FauxEngine::new());
        let refused =
            t.create_session_from("Understand the cache", Mode::Teach, &[], Some(&bundle));
        assert!(
            matches!(&refused, Err(TeachError::Invalid(why)) if why.contains("engineering context is off")),
            "{refused:?}"
        );
        assert!(
            t.engine().spawned.is_empty() && t.engine().inputs.is_empty(),
            "no agent, no input"
        );
        assert!(t.take_events().is_empty());
        let conn = noesis.store().connection();
        let rows: i64 = conn
            .query_row(
                "SELECT (SELECT count(*) FROM sessions) + (SELECT count(*) FROM evidence_bundles)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0, "nothing was stored");
        assert!(
            extract::engineering::material(conn, false, "ns_x").is_err(),
            "nothing is read"
        );
    }
    guard.assert_untouched();
}

#[test]
fn manual_engineering_extraction_queues_nothing_on_its_own() {
    let settings = NoesisSettings {
        extraction_engineering: EngineeringExtraction::Off,
        ..engineering_on()
    };
    let (_dir, noesis) = start(settings);
    let mut t = Teaching::new(&noesis, FauxEngine::new());
    let id = t
        .create_session_from(
            "Understand the cache",
            Mode::Teach,
            &[],
            Some(&bundle("127.0.0.1:9")),
        )
        .unwrap()
        .id;
    let conn = noesis.store().connection();
    let queued = extract::engineering::after_bundle_stored(conn, &noesis.effective(), &id).unwrap();
    assert!(queued.is_none());
    assert!(extract::jobs::list(conn, None).unwrap().is_empty());
}
