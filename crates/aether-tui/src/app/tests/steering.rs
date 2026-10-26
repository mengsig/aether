//! A steered message shows as pending until the agent has taken it (#303).
use super::composer::sent_messages;
use super::host_commands::app_with_host;
use super::*;
use aether_core::agent::WaitCondition;
use aether_core::id::{InboxItemId, MessageId};
use aether_core::inbox::{InboxItem, InboxKind};
use aether_core::model::{ContentBlock, MessageRole};

const CAPTION: &str = "lands at the next tool call";
const ANSWER: &str = "/answer 44 yes";
const SIMS: &str = "Are we running any sims? If so, please open them up on my screen";
const DRONEMIND: &str = "Ok, but also if we are running dronemind i would love to see that";

/// The daemon's side of one turn, numbered as it commits events.
struct Daemon {
    agent: AgentId,
    seq: u64,
    log: Vec<Event>,
}

impl Daemon {
    fn emit(&mut self, app: &mut App, kind: EventKind) {
        self.seq += 1;
        let event = event(self.seq, self.agent, kind);
        self.log.push(event.clone());
        app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
    }

    fn deliver(&mut self, app: &mut App, text: &str) -> InboxItemId {
        let item = InboxItem {
            id: InboxItemId::new(),
            agent_id: self.agent,
            seq: self.seq + 1,
            kind: InboxKind::Message {
                message_id: MessageId::new(),
                from: Actor::user(),
                content: vec![ContentBlock::text(text)],
            },
            delivered_at: now(),
        };
        let id = item.id;
        self.emit(app, EventKind::InboxDelivered { item });
        id
    }

    fn consume(&mut self, app: &mut App, items: Vec<InboxItemId>) {
        assert!(!items.is_empty(), "a consumption names its items");
        let up_to = self.seq;
        self.emit(app, EventKind::InboxConsumed { up_to, items });
    }

    /// The agent answers and its turn ends, waiting on its inbox.
    fn answer_and_wait(&mut self, app: &mut App, text: &str) {
        self.emit(
            app,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: vec![ContentBlock::text(text)],
                invocation: None,
            },
        );
        self.emit(
            app,
            EventKind::AgentWaiting {
                condition: WaitCondition::Inbox,
            },
        );
    }
}

fn running_agent() -> (App, Daemon) {
    let (mut app, agent) = app_with_host();
    app.tree.nodes.get_mut(&agent).unwrap().agent.status = AgentStatus::Running;
    let daemon = Daemon {
        agent,
        seq: 0,
        log: Vec::new(),
    };
    (app, daemon)
}

/// Enter on a running agent: the message is sent and previewed as steering.
fn steer(app: &mut App, agent: AgentId, text: &str) {
    let before = pending(app, agent).len();
    // The live clock ticks: a message is sent after every event seen so far.
    app.now = now() + chrono::Duration::milliseconds(1);
    app.input.set_text(text);
    let effects = app.reduce(key(KeyCode::Enter));
    assert_eq!(sent_messages(&effects), [(agent, text.to_owned())]);
    assert_eq!(
        pending(app, agent).len(),
        before + 1,
        "previewed as steering"
    );
}

fn pending(app: &App, agent: AgentId) -> Vec<String> {
    app.transcripts[&agent]
        .steering
        .iter()
        .map(|s| s.text.clone())
        .collect()
}

fn operator_cells(app: &App, agent: AgentId) -> Vec<String> {
    app.transcripts[&agent]
        .all_cells()
        .filter_map(|cell| match &cell.model {
            CellModel::User {
                from: crate::app::transcript::UserFrom::Operator,
                text,
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn reload_history(app: &mut App, daemon: &Daemon) {
    let agent = daemon.agent;
    app.transcripts.get_mut(&agent).unwrap().loaded = false;
    assert!(app.backfill_request(agent).is_some(), "history requested");
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(app, agent),
        result: Ok(Reply::Events(daemon.log.clone())),
    });
    assert!(app.transcripts[&agent].loaded);
}

/// `/answer` lands, then two messages steered while the turn runs. The
/// runtime records each consumed once the engine took it; the agent answers
/// both and its turn ends.
fn answer_then_two_steers(app: &mut App, daemon: &mut Daemon) -> [InboxItemId; 2] {
    let agent = daemon.agent;
    steer(app, agent, ANSWER);
    let answer = daemon.deliver(app, ANSWER);
    daemon.consume(app, vec![answer]);
    assert!(pending(app, agent).is_empty(), "{:?}", pending(app, agent));
    steer(app, agent, SIMS);
    steer(app, agent, DRONEMIND);
    let sims = daemon.deliver(app, SIMS);
    let dronemind = daemon.deliver(app, DRONEMIND);
    assert_eq!(
        pending(app, agent),
        [SIMS, DRONEMIND],
        "delivered, not taken"
    );
    assert!(crate::testing::render_to(app, 120, 40).contains(CAPTION));
    [sims, dronemind]
}

fn assert_nothing_pending(app: &App, agent: AgentId) {
    assert!(pending(app, agent).is_empty(), "{:?}", pending(app, agent));
    let screen = crate::testing::render_to(app, 120, 40);
    assert!(!screen.contains(CAPTION), "{screen}");
    assert_eq!(operator_cells(app, agent), [ANSWER, SIMS, DRONEMIND]);
}

#[test]
fn messages_steered_after_an_answer_leave_once_the_agent_took_them() {
    let (mut app, mut daemon) = running_agent();
    let [sims, dronemind] = answer_then_two_steers(&mut app, &mut daemon);
    daemon.consume(&mut app, vec![sims]);
    daemon.consume(&mut app, vec![dronemind]);
    daemon.answer_and_wait(&mut app, "Your two requests are now one standing rule");
    assert_nothing_pending(&app, daemon.agent);
}

#[test]
fn a_message_the_turn_did_not_take_shows_until_the_next_turn_takes_it() {
    let (mut app, mut daemon) = running_agent();
    let [sims, dronemind] = answer_then_two_steers(&mut app, &mut daemon);
    daemon.consume(&mut app, vec![sims]);
    daemon.answer_and_wait(&mut app, "No sims are running.");
    assert_eq!(
        pending(&app, daemon.agent),
        [DRONEMIND],
        "still undelivered"
    );
    daemon.emit(&mut app, EventKind::AgentStarted { turn: 2 });
    daemon.consume(&mut app, vec![dronemind]);
    assert_nothing_pending(&app, daemon.agent);
}

#[test]
fn a_history_reload_keeps_steering_retirable_and_never_brings_it_back() {
    let (mut app, mut daemon) = running_agent();
    let [sims, dronemind] = answer_then_two_steers(&mut app, &mut daemon);
    // Reloaded between delivery and consumption: the previews still retire.
    reload_history(&mut app, &daemon);
    assert_eq!(pending(&app, daemon.agent), [SIMS, DRONEMIND]);
    daemon.consume(&mut app, vec![sims, dronemind]);
    daemon.answer_and_wait(&mut app, "Your two requests are now one standing rule");
    assert_nothing_pending(&app, daemon.agent);
    // Reloaded after the turn: the same history shows nothing pending.
    reload_history(&mut app, &daemon);
    assert_nothing_pending(&app, daemon.agent);
}
