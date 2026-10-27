//! Recent history on attach and older windows on demand. Live frames arriving
//! during the initial request are replayed over the bounded history window.
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use aether_core::id::InvocationId;
use aether_core::time::Timestamp;
use aether_protocol::{Delta, EventQuery, Reply};

use super::transcript::{self, IngestCtx, StreamState};
use super::{AgentId, App, ClientError, Command, Effect, Event, Then};

/// One page; the daemon clamps larger requests to this.
const PAGE: usize = transcript::HISTORY_WINDOW;
/// Live events and deltas held while any backfill is in flight.
const MAX_BUFFERED: usize = 20_000;

#[derive(Debug, Default)]
pub(super) struct Backfill {
    /// Agents whose history is being fetched, with the pages so far.
    pages: HashMap<AgentId, Pending>,
    /// Live frames since the first request still in flight went out.
    buffered: Vec<Buffered>,
    /// `buffered` hit its bound: rebuilding would drop live cells.
    overflowed: bool,
}

#[derive(Debug)]
struct Pending {
    events: Vec<Event>,
    before: Option<u64>,
    older_available: bool,
    latest_requested: bool,
    after: u64,
    stream: StreamState,
    buffer_start: usize,
    abandoned: Option<Abandoned>,
    /// Asked for on this connection. History expiring cancels pages and bumps
    /// it, so a cancelled page's reply never matches its replacement.
    connection: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abandoned {
    Disconnected,
    Archived,
}

#[derive(Debug, Clone)]
enum Buffered {
    Event(Box<Event>),
    Delta {
        agent: AgentId,
        invocation: InvocationId,
        delta: Delta,
        after: u64,
        at: Timestamp,
    },
    Stream {
        agent: AgentId,
        state: StreamState,
        after: u64,
    },
}

impl Buffered {
    fn order(&self) -> (u64, u8) {
        // At one sequence, restore the boundary after its event and before
        // later deltas; stable sorting preserves delta arrival order.
        match self {
            Self::Event(event) => (event.seq, 0),
            Self::Stream { after, .. } => (*after, 1),
            Self::Delta { after, .. } => (*after, 2),
        }
    }
}

impl App {
    pub(crate) fn reading_history(&self) -> bool {
        self.selected
            .and_then(|agent| self.transcripts.get(&agent))
            .is_some_and(|transcript| transcript.live.is_some())
    }

    pub(crate) fn earlier_history_available(&self) -> bool {
        self.selected
            .and_then(|agent| self.transcripts.get(&agent))
            .is_some_and(|transcript| transcript.older_available)
    }

    /// The first page of `agent`'s history, unless it is loaded or loading.
    pub(super) fn backfill_request(&mut self, agent: AgentId) -> Option<Effect> {
        let loaded = self.transcripts.get(&agent).is_some_and(|t| t.loaded);
        if loaded || self.backfill.pages.contains_key(&agent) {
            return None;
        }
        self.history_request(agent, None)
    }

    pub(crate) fn older_history_request(&mut self, agent: AgentId) -> Option<Effect> {
        let current = self.transcripts.get(&agent)?;
        if !current.older_available {
            return None;
        }
        let before = current.history.front()?.seq;
        self.history_request(agent, Some(before))
    }

    pub(crate) fn latest_history_request(&mut self, agent: AgentId) -> Option<Effect> {
        if let Some(pending) = self.backfill.pages.get_mut(&agent) {
            pending.latest_requested |= pending.before.is_some();
        }
        let current = self.transcripts.get_mut(&agent)?;
        if let Some(mut live) = current.live.take() {
            live.queue = std::mem::take(&mut current.queue);
            live.steering = std::mem::take(&mut current.steering);
            *current = *live;
            self.dirty = true;
        }
        None
    }

    fn history_request(&mut self, agent: AgentId, before: Option<u64>) -> Option<Effect> {
        if self.backfill.pages.contains_key(&agent) {
            return None;
        }
        let current = self.transcripts.entry(agent).or_default();
        let stream = current.live_state().stream.clone();
        self.backfill.pages.insert(
            agent,
            Pending {
                events: Vec::new(),
                before,
                older_available: false,
                latest_requested: false,
                after: self.last_seq.unwrap_or(0),
                stream,
                buffer_start: self.backfill.buffered.len(),
                abandoned: None,
                connection: self.connection,
            },
        );
        Some(page_request(agent, before, self.connection))
    }

    /// A client attached mid-turn saw nothing sent: what a running turn has not
    /// taken yet is previewed again, whichever of history or tree came last.
    pub(super) fn restore_steering(&mut self) {
        for (agent, transcript) in &mut self.transcripts {
            let running = self.tree.nodes.get(agent).is_some_and(|node| {
                node.agent.status.kind() == aether_core::agent::AgentStatusKind::Running
            });
            if running {
                transcript::restore_steering(transcript);
            }
        }
    }

    /// Keep a live event for replay while any history is in flight.
    pub(super) fn backfill_note(&mut self, event: &Event) {
        if matches!(
            event.kind,
            super::EventKind::AgentArchived { archived: true }
        ) && let Some(pending) = event
            .agent_id
            .and_then(|agent| self.backfill.pages.get_mut(&agent))
        {
            pending.abandoned = Some(Abandoned::Archived);
        }
        if !self.backfill.pages.is_empty() && !self.backfill.overflowed {
            self.backfill_buffer(Buffered::Event(Box::new(event.clone())));
        }
    }

    pub(super) fn backfill_disconnected(&mut self) {
        for pending in self.backfill.pages.values_mut() {
            pending.abandoned.get_or_insert(Abandoned::Disconnected);
        }
    }

    pub(super) fn backfill_delta(
        &mut self,
        agent: AgentId,
        invocation: InvocationId,
        delta: &Delta,
    ) {
        if self.backfill.pages.contains_key(&agent) && !self.backfill.overflowed {
            self.backfill_buffer(Buffered::Delta {
                agent,
                invocation,
                delta: delta.clone(),
                after: self.last_seq.unwrap_or(0),
                at: self.now,
            });
        }
    }

    fn backfill_buffer(&mut self, frame: Buffered) {
        if self.backfill.pages.is_empty() {
            return;
        }
        if self.backfill.buffered.len() >= MAX_BUFFERED {
            self.backfill.overflowed = true;
            return;
        }
        self.backfill.buffered.push(frame);
    }

    pub(super) fn on_backfill(
        &mut self,
        agent: AgentId,
        connection: u64,
        result: Result<Reply, ClientError>,
    ) -> Vec<Effect> {
        let Some(mut pending) = self.take_page(agent, connection) else {
            return Vec::new();
        };
        if pending.abandoned.is_some() || pending.latest_requested {
            return self.finish_backfill(agent, pending, false);
        }
        let events = match result {
            Ok(Reply::Events(events)) => events,
            Ok(other) => {
                self.flash_error(format!(
                    "loading {}'s transcript: unexpected reply {other:?}; {}",
                    self.agent_name(agent),
                    retry_hint(pending.before)
                ));
                return self.finish_backfill(agent, pending, false);
            }
            Err(error) => {
                self.page_failed(agent, pending.before, &error);
                return self.finish_backfill(agent, pending, false);
            }
        };
        if self.backfill.overflowed && pending.before.is_none() {
            self.flash_error(format!("too much live activity while loading {}'s history; keeping live content; reselect to retry", self.agent_name(agent)));
            return self.finish_backfill(agent, pending, false);
        }
        if events.len() >= PAGE && events.windows(2).any(|pair| pair[0].seq < pair[1].seq) {
            self.flash_error("This daemon does not support recent history windows; upgrade the daemon to load history");
            return self.finish_backfill(agent, pending, false);
        }
        pending.older_available = events.len() >= PAGE;
        if events.is_empty() && pending.before.is_some() {
            if let Some(current) = self.transcripts.get_mut(&agent) {
                current.older_available = false;
            }
            // An empty older page leaves the displayed window intact.
            return self.finish_backfill(agent, pending, false);
        }
        pending.events = events;
        self.finish_backfill(agent, pending, true)
    }

    /// Say why the page before `before` did not load. Expired older history
    /// means no earlier page exists any more.
    fn page_failed(&mut self, agent: AgentId, before: Option<u64>, error: &ClientError) {
        debug_assert!(
            !self.backfill.pages.contains_key(&agent),
            "a failed page is no longer in flight"
        );
        let expired = error.expired_span().is_some()
            || matches!(error, ClientError::Protocol(reply)
            if reply.code == aether_protocol::ErrorCode::NotFound
                && reply.message.contains("history expired"));
        if before.is_none() || !expired {
            self.flash_error(format!(
                "loading {}'s transcript: {error}; {}",
                self.agent_name(agent),
                retry_hint(before)
            ));
            return;
        }
        if let Some(current) = self.transcripts.get_mut(&agent) {
            current.older_available = false;
        }
        self.flash_error(format!("Earlier history is no longer available: {error}"));
    }

    /// The page this reply answers. A page cancelled when history expired
    /// answers nothing, and the request that replaced it stays in flight.
    fn take_page(&mut self, agent: AgentId, connection: u64) -> Option<Pending> {
        debug_assert!(
            connection <= self.connection,
            "a reply from a connection not yet made"
        );
        let Entry::Occupied(slot) = self.backfill.pages.entry(agent) else {
            return None;
        };
        if slot.get().connection != connection {
            debug_assert!(
                slot.get().connection > connection,
                "a page in flight is never older than a reply it outlived"
            );
            return None;
        }
        Some(slot.remove())
    }

    /// Rebuild history without the current stream, then replay live frames in
    /// sequence order, including the stream that existed at the request boundary.
    fn finish_backfill(&mut self, agent: AgentId, pending: Pending, success: bool) -> Vec<Effect> {
        let buffered = if self.backfill.pages.is_empty() {
            self.backfill.overflowed = false;
            std::mem::take(&mut self.backfill.buffered)
        } else if success {
            self.backfill.buffered.clone()
        } else {
            Vec::new()
        };
        if !success {
            if pending.before.is_none()
                && let Some(current) = self.transcripts.get_mut(&agent)
            {
                current.loaded = false;
            }
            let retry = pending.abandoned == Some(Abandoned::Disconnected)
                && self.connected
                && self.selected == Some(agent);
            return if retry {
                self.backfill_request(agent).into_iter().collect()
            } else {
                Vec::new()
            };
        }
        let current = self.transcripts.entry(agent).or_default();
        let generation = current.stream.generation.wrapping_add(1);
        let client_errors = current.client_errors();
        let mut rebuilt = transcript::Transcripts::new();
        rebuilt.insert(agent, current.continuing());
        let mut frames: Vec<_> = pending
            .events
            .into_iter()
            .map(|event| Buffered::Event(Box::new(event)))
            .collect();
        if pending.before.is_none() {
            frames.push(Buffered::Stream {
                agent,
                state: pending.stream,
                after: pending.after,
            });
            frames.extend(buffered.into_iter().skip(pending.buffer_start));
        }
        frames.sort_by_key(Buffered::order);
        let mut unlisted = false;
        for frame in &frames {
            if let Buffered::Event(event) = frame {
                unlisted |= self.note_unlisted_schedule(event);
            }
        }
        let ctx = IngestCtx {
            tree: &self.tree,
            now: self.now,
        };
        let mut seen = HashSet::new();
        for frame in frames {
            match frame {
                Buffered::Event(event) if event.seq == 0 || seen.insert(event.seq) => {
                    transcript::ingest_replay_event(&mut rebuilt, &event, &ctx);
                }
                Buffered::Event(_) => {}
                Buffered::Stream { agent, state, .. } => {
                    rebuilt.entry(agent).or_default().stream = state;
                }
                Buffered::Delta {
                    agent,
                    invocation,
                    delta,
                    at,
                    ..
                } => {
                    rebuilt
                        .entry(agent)
                        .or_default()
                        .stream(invocation, &delta, at);
                }
            }
        }
        if let Some(mut fresh) = rebuilt.remove(&agent) {
            fresh.retain_client_errors(client_errors);
            fresh.loaded = true;
            fresh.older_available = pending.older_available;
            if pending.before.is_some() {
                #[expect(
                    clippy::expect_used,
                    reason = "the requested transcript was seeded above"
                )]
                let mut current = self
                    .transcripts
                    .remove(&agent)
                    .expect("requested transcript exists");
                current.queue.clear();
                current.steering.clear();
                fresh.live = Some(current.live.take().unwrap_or_else(|| Box::new(current)));
                fresh.epoch = Default::default();
            }
            fresh.stream.generation = generation;
            transcript::replace_transcript(&mut self.transcripts, agent, fresh, &ctx);
            if pending.before.is_some() {
                self.scroll_log
                    .push(agent, transcript::ScrollIntent::Latest);
            }
        }
        transcript::bound_history(&mut self.transcripts, &ctx);
        self.restore_steering();
        if pending.before.is_some() {
            self.flash("Reading older history; Ctrl+End returns to latest");
        }
        self.dirty = true;
        if unlisted {
            return self.schedule_names_request().into_iter().collect();
        }
        Vec::new()
    }
}

fn retry_hint(before: Option<u64>) -> &'static str {
    if before.is_some() {
        "scroll up to retry"
    } else {
        "reselect to retry"
    }
}

fn page_request(agent: AgentId, before: Option<u64>, connection: u64) -> Effect {
    Effect::Request {
        command: Command::GetEvents {
            after: 0,
            limit: PAGE,
            query: EventQuery {
                agent: Some(agent),
                before: Some(before.unwrap_or(u64::MAX)),
                // Child activity the transcript never shows would crowd the
                // agent's own turns off the page.
                child_types: transcript::PARENT_VISIBLE_CHILD_TYPES
                    .map(String::from)
                    .into(),
                ..EventQuery::default()
            },
        },
        then: Then::Backfill { agent, connection },
    }
}
