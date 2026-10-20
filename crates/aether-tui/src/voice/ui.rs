use std::collections::VecDeque;
use std::path::PathBuf;

use aether_core::id::AgentId;

use crate::app::transcript::CellModel;
use crate::app::{App, Effect};
use crate::prefs::{UiPrefs, VoicePrefs};

use super::{Action, CheckFailure, Effect as VoiceEffect, Missing, ModelKind, Reply, backend};

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;

#[path = "reply_ui.rs"]
mod reply_ui;

#[path = "ui/preferences.rs"]
mod preferences;
#[path = "ui/replies.rs"]
mod replies;
#[path = "ui/setup_actions.rs"]
mod setup_actions;

const QUEUE_LIMIT: usize = 3;

#[derive(Debug)]
struct Capture {
    id: u64,
    agent: AgentId,
    selection: u64,
    revision: u64,
    draft: String,
    cursor: usize,
    phase: &'static str,
    cancelled: bool,
}

#[derive(Debug, Clone)]
struct Question {
    agent: AgentId,
    seq: u64,
    text: String,
    expires: u64,
}

#[derive(Debug)]
struct Speech {
    id: u64,
    question: Question,
    cancelled: bool,
    playing: bool,
}

/// A backend install in flight, started from the `/voice` confirm view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Install {
    id: u64,
    model: ModelKind,
    /// Latest human-readable progress line.
    stage: String,
    /// A `Cancel` went out; guards against sending a second one.
    cancelled: bool,
}

#[derive(Debug, Default)]
pub struct State {
    pub available: bool,
    /// Speaking questions aloud right now. Transient: a mute, a suspend or a
    /// disconnect clears it without touching `saved`.
    pub enabled: bool,
    pub selection: u64,
    pub watermark: u64,
    replies: reply_ui::ReplyState,
    next_id: u64,
    checking: Option<(u64, bool)>,
    capture: Option<Capture>,
    recovered: Option<(AgentId, String)>,
    speech: Option<Speech>,
    queue: VecDeque<Question>,
    pub(crate) install: Option<Install>,
    /// What `ui.toml` last recorded. The file stays the source of truth;
    /// this is the copy the popup shows and every save updates.
    pub(crate) saved: VoicePrefs,
    /// A saved opt-in this run has not applied yet. Any explicit user toggle
    /// clears it, so it can never resurrect a state the user just left.
    restore: Option<VoicePrefs>,
    /// The in-flight check that restoration started, matched by id so a late
    /// result is never mistaken for a user-requested one.
    restoring: Option<u64>,
    /// The user changed a voice setting this run, so the saved setting must
    /// not be re-applied behind their back.
    overridden: bool,
    /// A saved setting was restored this run; `/voice status` says so.
    restored: bool,
    /// The installable backend the latest failed check lacked, with that
    /// check's id. A passing check or a finished install clears it.
    missing: Option<(u64, Missing)>,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn status(&self) -> Option<&'static str> {
        if self.install.is_some() {
            return Some("voice installing /voice cancel");
        }
        if let Some(status) = self.replies.status() {
            return Some(status);
        }
        if let Some(capture) = &self.capture {
            return Some(capture.phase);
        }
        if self.speech.is_some() {
            return Some("voice speaking /voice mute");
        }
        if self.checking.is_some() {
            return Some("voice checking");
        }
        if self.recovered.is_some() {
            return Some("voice draft F5 insert");
        }
        if self.replies.enabled {
            return Some("voice replies on: global reply records / sends without review");
        }
        self.enabled.then_some("voice questions on")
    }

    /// Progress of the running install of `model`, if that is what runs.
    pub fn install_stage(&self, model: ModelKind) -> Option<&str> {
        self.install
            .as_ref()
            .filter(|install| install.model == model)
            .map(|install| install.stage.as_str())
    }

    /// Id of the latest check that found a backend missing; 0 before any.
    pub fn missing_serial(&self) -> u64 {
        self.missing.map_or(0, |(id, _)| id)
    }

    /// The install row a check newer than `seen` asks the popup to select.
    pub fn install_lead(&self, seen: u64) -> Option<ModelKind> {
        let (id, missing) = self.missing?;
        (id > seen).then(|| missing.install_row())
    }

    pub fn recording(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|c| c.phase == "voice recording F4 stop")
    }
}

impl App {
    pub(crate) fn voice_suspend(&mut self) -> Vec<Effect> {
        self.voice.queue.clear();
        self.cancel_pending_enable();
        let mut effects = self.disarm_voice_replies();
        effects.extend(self.voice_action(Action::Cancel));
        effects.extend(self.cancel_speech());
        effects
    }

    #[cfg(unix)]
    pub(crate) fn voice_resume(&mut self, replies_enabled: bool) {
        // Resume the opt-in, never cancelled audio or an old armed question.
        self.voice.replies.enabled = replies_enabled;
    }

    pub(crate) fn voice_key_hint(&self, text: &str) -> String {
        use crate::keys::KeyAction;
        let dictate = self.keys.help_chord(&[KeyAction::Dictate], true);
        let insert = self.keys.help_chord(&[KeyAction::InsertDictation], true);
        text.split("F4")
            .map(|part| part.replace("F5", &insert))
            .collect::<Vec<_>>()
            .join(&dictate)
    }

    fn voice_dir(&self) -> Option<PathBuf> {
        self.options.prefs_dir.clone()
    }

    pub(crate) fn voice_action(&mut self, action: Action) -> Vec<Effect> {
        if self.options.is_remote() {
            self.flash_error("voice is local-only; run aether --local on the host instead");
            return Vec::new();
        }
        if !self.voice.available {
            self.flash(
                "local voice requires the interactive TUI on Linux or macOS; see docs/voice.md",
            );
            return Vec::new();
        }
        match action {
            Action::Status => {
                let state = format!(
                    "{}; global reply: {}",
                    self.voice_key_hint(self.voice.status().unwrap_or("voice off")),
                    self.voice_reply_diagnostic()
                );
                let restored = if self.voice.restored {
                    " (restored from saved settings this run)"
                } else {
                    ""
                };
                let saved = saved_label(self.voice.saved);
                let dictate = self
                    .keys
                    .help_chord(&[crate::keys::KeyAction::Dictate], true);
                let insert = self
                    .keys
                    .help_chord(&[crate::keys::KeyAction::InsertDictation], true);
                self.flash(format!(
                    "{state}{restored}; next launch: {saved}; {dictate} record/stop; {insert} insert recovered draft; /voice opens settings and installs backends; check, on, off (saved), mute (this run only), replies on/off, cancel, insert; docs/voice.md"
                ));
                Vec::new()
            }
            Action::RepliesOn => self.enable_voice_replies(),
            Action::RepliesOff => {
                self.keep_question_restoration();
                let effects = self.disarm_voice_replies();
                let questions = self.questions_intended();
                let saved = self.persist_voice(VoicePrefs {
                    questions,
                    replies: false,
                });
                self.flash(self.voice_key_hint(&format!(
                    "global voice replies off{saved}; question speech and F4 reviewed dictation unchanged"
                )));
                effects
            }
            Action::Check | Action::On => self.voice_check(action == Action::On),
            Action::Start => self.voice_start(),
            Action::Setup => self.open_view(crate::ui::views::ViewKind::Voice),
            Action::Stop => {
                if let Some(capture) = self.voice.capture.as_mut() {
                    if capture.cancelled {
                        return Vec::new();
                    }
                    capture.phase = "voice transcribing /voice cancel";
                    vec![Effect::Voice(VoiceEffect::Stop(capture.id))]
                } else {
                    self.flash(
                        self.voice_key_hint("no recording to stop; F4 starts local recording"),
                    );
                    Vec::new()
                }
            }
            Action::Cancel => {
                self.voice.recovered = None;
                let mut effects = self.cancel_voice_reply();
                effects.extend(self.cancel_install());
                if let Some(capture) = self.voice.capture.as_mut() {
                    capture.cancelled = true;
                    capture.phase = "voice cancelling";
                    effects.push(Effect::Voice(VoiceEffect::Cancel(capture.id)));
                } else if effects.is_empty() && self.voice.replies.active.is_none() {
                    self.flash("voice recording/draft cancelled");
                }
                effects
            }
            Action::Off => {
                let effects = self.voice_quiet();
                let saved = self.persist_voice(VoicePrefs::default());
                self.flash(self.voice_key_hint(&format!(
                    "voice off: question speech stopped and global replies disarmed, now and at the next launch{saved}; F4 dictation unchanged"
                )));
                effects
            }
            Action::Mute => {
                let saved = saved_label(self.voice.saved);
                let effects = self.voice_quiet();
                self.flash(self.voice_key_hint(&format!(
                    "question speech muted for this run; global replies disarmed; saved setting ({saved}) is unchanged but is not re-applied until the next launch; F4 dictation unchanged"
                )));
                effects
            }
            Action::Insert => self.insert_voice_draft(),
        }
    }

    fn voice_check(&mut self, enable: bool) -> Vec<Effect> {
        if self.voice.checking.is_some() || self.voice.replies.checking.is_some() {
            self.flash("voice configuration check already running");
            return Vec::new();
        }
        let Some(dir) = self.voice_dir() else {
            self.flash("voice configuration directory unavailable; see docs/voice.md");
            return Vec::new();
        };
        let id = self.voice.id();
        self.voice.checking = Some((id, enable));
        vec![Effect::Voice(VoiceEffect::Check {
            id,
            dir,
            speech: enable,
        })]
    }

    fn voice_start(&mut self) -> Vec<Effect> {
        if self.voice.replies.active.is_some() {
            self.flash(self.voice_key_hint("global voice reply active; use its global shortcut to stop/send or cancel; F4 is separate"));
            return Vec::new();
        }
        if self.voice.capture.is_some() {
            self.flash(self.voice_key_hint(
                "voice capture already active; F4 stops recording, /voice cancel discards it",
            ));
            return Vec::new();
        }
        let (Some(agent), Some(dir)) = (self.selected, self.voice_dir()) else {
            self.flash(
                "select an agent and configure voice.toml before recording; /voice setup installs a backend",
            );
            return Vec::new();
        };
        if !self.connected {
            self.flash("connect to the daemon before recording");
            return Vec::new();
        }
        let id = self.voice.id();
        self.voice.recovered = None;
        self.voice.queue.clear();
        self.voice.capture = Some(Capture {
            id,
            agent,
            selection: self.voice.selection,
            revision: self.input.revision,
            draft: self.input.text.clone(),
            cursor: self.input.cursor,
            phase: "voice recording F4 stop",
            cancelled: false,
        });
        let mut effects = self.cancel_speech();
        effects.push(Effect::Voice(VoiceEffect::Record { id, dir }));
        effects
    }

    fn insert_voice_draft(&mut self) -> Vec<Effect> {
        if crate::viewer::hides_composer(self) {
            self.flash("voice draft retained: return to chat before inserting");
            return Vec::new();
        }
        let Some((agent, _)) = &self.voice.recovered else {
            self.flash("no recovered voice draft");
            return Vec::new();
        };
        if self.selected != Some(*agent) {
            self.flash("select the recording's original agent before /voice insert");
            return Vec::new();
        }
        if let Some((_, text)) = self.voice.recovered.take() {
            self.input.insert_str(&text);
            self.flash("voice draft inserted; review before sending");
        }
        Vec::new()
    }

    pub(crate) fn voice_question(&mut self, agent: AgentId, seq: u64, text: &str) {
        if seq <= self.voice.watermark {
            return;
        }
        self.voice.watermark = seq;
        if !self.voice.enabled
            || !self.connected
            || self.voice.capture.is_some()
            || self.voice.replies.active.is_some()
        {
            return;
        }
        let text = format!("{} asks: {}", self.agent_name(agent), text);
        if text.len() > backend::QUESTION_LIMIT
            || text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            self.flash("question not spoken: exceeds 2048 bytes or contains control characters; read its card");
            return;
        }
        if self.voice.queue.len() == QUEUE_LIMIT {
            self.flash("voice queue full; newest question remains available in its card");
            return;
        }
        self.voice.queue.push_back(Question {
            agent,
            seq,
            text,
            expires: self.tick + 30 * crate::app::TICKS_PER_SECOND,
        });
    }

    fn question_live(&self, question: &Question) -> bool {
        self.question_pending(question.agent, question.seq)
    }

    /// The question `seq` is still unanswered, its agent can still take the
    /// answer, and nothing the person said since has overtaken it.
    fn question_pending(&self, agent: AgentId, seq: u64) -> bool {
        let awaiting = self.transcripts.get(&agent).is_some_and(|t| {
            let t = t.live_state();
            let Some(index) = t.cells.iter().position(|cell| {
                matches!(cell.model, CellModel::Question { seq: s, state: crate::app::transcript::QuestionState::Pending, .. } if s == seq)
            }) else {
                return false;
            };
            !t.cells[index + 1..]
                .iter()
                .any(|cell| matches!(cell.model, CellModel::User { .. }))
        });
        awaiting
            && !self
                .tree
                .nodes
                .get(&agent)
                .is_some_and(|n| n.agent.status.is_terminal() || n.agent.archived)
    }

    fn cancel_speech(&mut self) -> Vec<Effect> {
        if let Some(speech) = &mut self.voice.speech
            && !speech.cancelled
        {
            speech.cancelled = true;
            return vec![Effect::Voice(VoiceEffect::Cancel(speech.id))];
        }
        Vec::new()
    }

    pub(crate) fn voice_reconcile(&mut self) -> Vec<Effect> {
        if self.options.is_remote() {
            return Vec::new();
        }
        let mut effects = self.restore_voice_prefs();
        effects.extend(self.reconcile_voice_replies());
        if !self.connected || self.quit_requested {
            effects.extend(self.cancel_install());
            self.voice.queue.clear();
            self.cancel_pending_enable();
            if let Some(capture) = &mut self.voice.capture
                && !capture.cancelled
            {
                capture.cancelled = true;
                capture.phase = "voice cancelling";
                effects.push(Effect::Voice(VoiceEffect::Cancel(capture.id)));
            }
            self.voice.recovered = None;
        }
        let stale = self.voice.speech.as_ref().is_some_and(|s| {
            !self.question_live(&s.question) || (!s.playing && self.tick >= s.question.expires)
        });
        if !self.connected || self.quit_requested || !self.voice.enabled {
            self.voice.queue.clear();
        }
        if !self.connected || self.quit_requested || !self.voice.enabled || stale {
            effects.extend(self.cancel_speech());
        }
        if self.voice.speech.is_some()
            || self.voice.capture.is_some()
            || self.voice.replies.active.is_some()
            || !self.voice.enabled
            || !self.connected
        {
            return effects;
        }
        while let Some(question) = self.voice.queue.pop_front() {
            if self.tick >= question.expires || !self.question_live(&question) {
                continue;
            }
            let Some(dir) = self.voice_dir() else {
                break;
            };
            self.voice.replies.armed = None;
            let id = self.voice.id();
            effects.push(Effect::Voice(VoiceEffect::Synthesize {
                id,
                dir,
                text: question.text.clone(),
            }));
            self.voice.speech = Some(Speech {
                id,
                question,
                cancelled: false,
                playing: false,
            });
            break;
        }
        effects
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// What the next launch will restore, for notices and the popup.
fn saved_label(saved: VoicePrefs) -> String {
    format!(
        "questions {}, replies {}",
        on_off(saved.questions),
        on_off(saved.replies)
    )
}
