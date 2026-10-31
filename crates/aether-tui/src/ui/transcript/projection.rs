use crate::app::{
    App,
    transcript::{AgentTranscript, CellId, TranscriptEpoch},
};
use aether_core::id::AgentId;
use std::sync::Arc;

pub struct Projection {
    pub committed: Arc<Vec<usize>>,
    pub pending: Arc<Vec<CellId>>,
}

#[derive(Debug, Default)]
pub struct VisibleIndex {
    key: Option<(AgentId, TranscriptEpoch, CellId, u32, bool, bool)>,
    revision: u64,
    positions: Arc<Vec<usize>>,
    pending: Arc<Vec<CellId>>,
}

impl VisibleIndex {
    pub fn project(
        &mut self,
        app: &App,
        agent: AgentId,
        transcript: &AgentTranscript,
    ) -> Projection {
        let policy = app.policy(Some(agent));
        let named = policy.definition.is_some() || app.details && app.view_mode != "standard";
        let key = (
            agent,
            transcript.epoch,
            transcript.id_floor,
            app.presentation_revision,
            named,
            app.details,
        );
        let revision = transcript.committed_revision();
        let valid_journal = revision == self.revision
            || transcript
                .changes
                .front()
                .is_some_and(|(seq, _)| *seq <= self.revision + 1)
                && transcript
                    .changes
                    .back()
                    .is_some_and(|(seq, _)| *seq == revision)
                && transcript
                    .changes
                    .iter()
                    .filter(|(seq, _)| *seq > self.revision)
                    .all(|(_, id)| id.is_some());
        let shows = |at: usize| {
            let cell = &transcript.cells[at];
            (named || !transcript.named_only.contains(&cell.id)) && policy.shows(&cell.model)
        };
        if self.key != Some(key) || !valid_journal {
            self.positions = Arc::new(
                (0..transcript.cells.len())
                    .filter(|at| shows(*at))
                    .collect(),
            );
            self.pending = Arc::new(if named {
                transcript
                    .pending_messages
                    .values()
                    .filter(|cell| policy.shows(&cell.model))
                    .map(|cell| cell.id)
                    .collect()
            } else {
                Vec::new()
            });
        } else if revision != self.revision {
            let positions = Arc::make_mut(&mut self.positions);
            let pending = Arc::make_mut(&mut self.pending);
            for (_, id) in transcript
                .changes
                .iter()
                .filter(|(seq, _)| *seq > self.revision)
            {
                let Some(id) = *id else {
                    continue;
                };
                if let Some(at) = transcript.position(id) {
                    match (positions.binary_search(&at), shows(at)) {
                        (Err(index), true) => positions.insert(index, at),
                        (Ok(index), false) => {
                            positions.remove(index);
                        }
                        _ => {}
                    }
                }
                let visible_pending = named
                    && transcript
                        .pending_messages
                        .get(&id)
                        .is_some_and(|cell| policy.shows(&cell.model));
                match (pending.binary_search(&id), visible_pending) {
                    (Err(index), true) => pending.insert(index, id),
                    (Ok(index), false) => {
                        pending.remove(index);
                    }
                    _ => {}
                }
            }
        }
        self.key = Some(key);
        self.revision = revision;
        Projection {
            committed: Arc::clone(&self.positions),
            pending: Arc::clone(&self.pending),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::transcript::{CallStatus, Cell, CellModel, UserFrom};

    #[test]
    fn rebuilding_at_the_same_revision_recomputes_visible_positions() {
        let mut app = crate::testing::app_with_transcript(&[]);
        app.view_mode = "focus".into();
        let agent = app.selected.unwrap();
        app.transcripts.insert(agent, AgentTranscript::default());
        app.transcripts.get_mut(&agent).unwrap().push_for_test(
            app.now,
            CellModel::Reasoning {
                title: None,
                text: "hidden reasoning".into(),
                duration_ms: None,
            },
        );
        let mut cache = VisibleIndex::default();
        let before = &app.transcripts[&agent];
        let revision = before.committed_revision();
        assert!(cache.project(&app, agent, before).committed.is_empty());
        let mut fresh = before.continuing();
        fresh.push_for_test(
            app.now,
            CellModel::Assistant {
                text: "visible reply".into(),
            },
        );
        assert_eq!(fresh.committed_revision(), revision);
        app.transcripts.insert(agent, fresh);
        assert_eq!(
            &*cache
                .project(&app, agent, &app.transcripts[&agent])
                .committed,
            &[0]
        );
    }

    #[test]
    fn removing_an_old_local_notice_recomputes_visible_positions() {
        let mut app = crate::testing::app_with_transcript(&[]);
        app.view_mode = "focus".into();
        let agent = app.selected.unwrap();
        for n in 0..64 {
            app.transcripts
                .get_mut(&agent)
                .unwrap()
                .client_error(app.now, format!("error {n}"));
            if n == 0 {
                app.transcripts.get_mut(&agent).unwrap().push_for_test(
                    app.now,
                    CellModel::Reasoning {
                        title: None,
                        text: "hidden reasoning".into(),
                        duration_ms: None,
                    },
                );
            }
        }
        let mut cache = VisibleIndex::default();
        assert_eq!(
            cache
                .project(&app, agent, &app.transcripts[&agent])
                .committed
                .len(),
            64
        );
        app.transcripts
            .get_mut(&agent)
            .unwrap()
            .client_error(app.now, "new error".into());
        let projection = cache.project(&app, agent, &app.transcripts[&agent]);
        assert_eq!(&*projection.committed, &(1..65).collect::<Vec<_>>());
    }

    #[test]
    fn a_pending_backlog_reuses_its_index_and_promotes_without_duplicate_rows() {
        use crate::app::AppEvent;
        use aether_client::SubscriptionFrame;
        use aether_core::event::{Actor, Event, EventDraft, EventKind};
        use aether_core::id::{EventId, InboxItemId, MessageId};
        use aether_core::inbox::{InboxItem, InboxKind};
        use aether_core::model::ContentBlock;
        let mut app = crate::testing::app_with_transcript(&[]);
        app.view_mode = "focus".into();
        let agent = app.selected.unwrap();
        let transcript = app.transcripts.get_mut(&agent).unwrap();
        for _ in 0..10_000 {
            let id = transcript.next_cell_id();
            transcript.pending_messages.insert(
                id,
                Cell {
                    id,
                    revision: Default::default(),
                    at: app.now,
                    model: CellModel::User {
                        from: UserFrom::Message {
                            from: "tester".into(),
                        },
                        text: "waiting".into(),
                        images: Vec::new(),
                    },
                },
            );
        }
        let mut cache = VisibleIndex::default();
        let first = cache.project(&app, agent, &app.transcripts[&agent]);
        let next = cache.project(&app, agent, &app.transcripts[&agent]);
        assert_eq!(first.pending.len(), 10_000);
        assert!(Arc::ptr_eq(&first.pending, &next.pending));
        drop(first);
        drop(next);
        let item = InboxItemId::new();
        let events = [
            EventKind::InboxDelivered {
                item: InboxItem {
                    id: item,
                    agent_id: agent,
                    seq: 1,
                    delivered_at: app.now,
                    kind: InboxKind::Message {
                        message_id: MessageId::new(),
                        from: Actor::agent(AgentId::new()),
                        content: vec![ContentBlock::text("new message")],
                    },
                },
            },
            EventKind::InboxConsumed {
                up_to: 1,
                items: vec![item],
            },
        ];
        for (index, kind) in events.into_iter().enumerate() {
            let event = Event::from_draft(
                EventDraft::new(Actor::System, kind).agent(agent),
                EventId::new(),
                index as u64 + 1,
                app.now,
            );
            app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event))));
            let projection = cache.project(&app, agent, &app.transcripts[&agent]);
            assert_eq!(
                projection.pending.len(),
                if index == 0 { 10_001 } else { 10_000 }
            );
            assert_eq!(projection.committed.len(), index);
        }
    }

    #[test]
    fn warm_frames_and_hidden_append_tails_keep_the_cached_projection() {
        let mut app = crate::testing::app_with_transcript(&[]);
        app.view_mode = "focus".into();
        let agent = app.selected.unwrap();
        let hidden = || CellModel::Exec {
            call: None,
            command: "ls".into(),
            status: CallStatus::Succeeded,
            output: String::new(),
            exit: Some(0),
            duration_ms: None,
            is_search: false,
            artifacts: vec![],
        };
        for _ in 0..10_000 {
            app.transcripts
                .get_mut(&agent)
                .unwrap()
                .push_for_test(app.now, hidden());
        }
        let mut cache = VisibleIndex::default();
        let first = cache
            .project(&app, agent, &app.transcripts[&agent])
            .committed;
        assert!(first.is_empty());
        let next = cache
            .project(&app, agent, &app.transcripts[&agent])
            .committed;
        assert!(Arc::ptr_eq(&first, &next));
        drop(first);
        drop(next);
        for _ in 0..10 {
            app.transcripts
                .get_mut(&agent)
                .unwrap()
                .push_for_test(app.now, hidden());
        }
        assert!(
            cache
                .project(&app, agent, &app.transcripts[&agent])
                .committed
                .is_empty()
        );
        let id = app.transcripts.get_mut(&agent).unwrap().push_for_test(
            app.now,
            CellModel::Assistant {
                text: "visible".into(),
            },
        );
        let t = &app.transcripts[&agent];
        assert_eq!(
            &*cache.project(&app, agent, t).committed,
            &[t.position(id).unwrap()]
        );
        for _ in 0..300 {
            app.transcripts
                .get_mut(&agent)
                .unwrap()
                .push_for_test(app.now, hidden());
        }
        assert_eq!(
            cache
                .project(&app, agent, &app.transcripts[&agent])
                .committed
                .len(),
            1
        );
    }
}
