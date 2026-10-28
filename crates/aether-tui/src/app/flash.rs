//! Hint-row feedback: timed notices and errors kept until the next key.

use aether_core::time::Timestamp;

use super::App;

pub const FLASH_MS: i64 = 2_000;
/// Longer flashes (errors) stay long enough to read, up to this.
const FLASH_MAX_MS: i64 = 6_000;
const READ_MS_PER_CHAR: i64 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub text: String,
    pub until: Timestamp,
}

impl Flash {
    pub(super) fn until_key(&self) -> bool {
        self.until == Timestamp::MAX_UTC
    }
}

impl App {
    /// Show `text` in the hint row until it has had time to be read.
    pub(crate) fn flash(&mut self, text: impl Into<String>) {
        let text = text.into();
        let chars = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        let ms = chars
            .saturating_mul(READ_MS_PER_CHAR)
            .clamp(FLASH_MS, FLASH_MAX_MS);
        self.flash = Some(Flash {
            text,
            until: self.now + chrono::Duration::milliseconds(ms),
        });
        self.dirty = true;
    }

    pub(super) fn keep_flash_until_key(&mut self) {
        if let Some(flash) = &mut self.flash {
            // The reducer and frame scheduler both use this deadline.
            flash.until = Timestamp::MAX_UTC;
        }
        debug_assert!(self.flash.as_ref().is_some_and(|flash| flash.until_key()));
        debug_assert!(self.live_flash().is_some());
    }

    /// Keep failed client operations in the transcript after the hint expires.
    pub(crate) fn flash_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.pending_client_errors.push((self.now, text.clone()));
        self.flush_client_errors();
        self.flash(text);
    }

    /// Flash `text` and keep it as a transcript row, so a narrow hint row cannot hide it.
    pub(crate) fn echo(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(agent) = self.selected {
            let at = self.now;
            self.transcripts
                .entry(agent)
                .or_default()
                .echo(at, text.clone());
        }
        self.flash(text);
    }

    pub(super) fn flush_client_errors(&mut self) {
        if self.pending_client_errors.is_empty() {
            return;
        }
        let Some(agent) = self.selected else {
            return;
        };
        let transcript = self.transcripts.entry(agent).or_default();
        for (at, text) in std::mem::take(&mut self.pending_client_errors) {
            transcript.client_error(at, text);
        }
    }

    /// The flash still showing at `self.now`.
    pub fn live_flash(&self) -> Option<&str> {
        self.flash
            .as_ref()
            .filter(|f| f.until > self.now)
            .map(|f| f.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::transcript::{CellModel, NoticeKind};
    use crate::app::{AppEvent, Then};
    use aether_protocol::Reply;

    #[test]
    fn client_failures_survive_flash_expiry_and_history_loading() {
        let mut app = crate::testing::app_with_transcript(&["hello", "answer"]);
        let agent = app.selected.expect("agent");
        app.transcripts.get_mut(&agent).expect("transcript").loaded = false;
        app.select(agent);
        app.reduce(AppEvent::EffectFailed {
            action: "clipboard copy".into(),
            error: "terminal write failed".into(),
        });
        app.reduce(AppEvent::Reply {
            then: Then::Ignore,
            result: Err(aether_client::ClientError::Disconnected),
        });
        let before: Vec<_> = app.transcripts[&agent]
            .cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.model,
                    CellModel::Notice {
                        kind: NoticeKind::ClientError,
                        ..
                    }
                )
            })
            .cloned()
            .collect();
        assert_eq!(before.len(), 2);
        app.now += chrono::Duration::seconds(7);
        assert!(app.live_flash().is_none());
        app.reduce(AppEvent::Reply {
            then: crate::testing::backfill_then(&app, agent),
            result: Ok(Reply::Events(Vec::new())),
        });
        for cell in &before {
            assert_eq!(
                app.transcripts[&agent].cell(cell.id),
                Some(cell),
                "error identity survives replay"
            );
        }
        let frame = crate::testing::render_to(&app, 120, 40);
        assert!(
            frame.contains("clipboard copy failed: terminal write failed"),
            "{frame}"
        );
        let plain = before
            .iter()
            .flat_map(crate::plain::describe)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains("clipboard copy failed"));
        assert!(plain.contains("did not complete"));
    }

    #[test]
    fn pause_and_resume_leave_a_transcript_row_at_80_columns() {
        let mut app = crate::testing::app_with_transcript(&["hello", "answer"]);
        let agent = app.selected.expect("agent");
        for (then, reply, expect) in [
            (
                Then::Paused(agent),
                Reply::Paused {
                    agents: vec![app.tree.nodes[&agent].agent.clone()],
                    mid_turn: Vec::new(),
                },
                "no turns until resumed",
            ),
            (
                Then::Resumed(agent),
                Reply::Agents(vec![app.tree.nodes[&agent].agent.clone()]),
                "resumed ",
            ),
        ] {
            app.reduce(AppEvent::Reply {
                then,
                result: Ok(reply),
            });
            app.now += chrono::Duration::seconds(7);
            assert!(app.live_flash().is_none(), "the hint row has long expired");
            let frame = crate::testing::render_to(&app, 80, 30);
            assert!(frame.contains(expect), "{expect:?} missing:\n{frame}");
        }
    }

    #[test]
    fn failure_before_agent_selection_is_kept_when_an_agent_is_adopted() {
        let mut app = App::new(Default::default());
        app.reduce(AppEvent::EffectFailed {
            action: "startup".into(),
            error: "offline".into(),
        });
        assert_eq!(app.pending_client_errors.len(), 1);
        let fixture = crate::testing::populated_app();
        let agent = fixture.selected.expect("agent");
        app.selected = Some(agent);
        app.reduce(AppEvent::Focus(true));
        assert!(app.pending_client_errors.is_empty());
        assert!(app.transcripts[&agent].cells.iter().any(|cell| matches!(
            &cell.model, CellModel::Notice { kind: NoticeKind::ClientError, text } if text.as_str() == "startup failed: offline"
        )));
    }
}
