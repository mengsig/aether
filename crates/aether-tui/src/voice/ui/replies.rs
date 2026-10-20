use super::{App, CheckFailure, Effect, Reply, VoiceEffect, VoicePrefs};

impl App {
    pub(crate) fn voice_reply(&mut self, reply: Reply) -> Vec<Effect> {
        self.dirty = true;
        match reply {
            Reply::Checked { id, result } => {
                self.note_missing_backend(id, &result);
                if self.voice.replies.checking == Some(id) {
                    let restored = self.voice.restoring == Some(id);
                    if restored {
                        self.voice.restoring = None;
                    }
                    self.voice_replies_checked(restored, result);
                    return Vec::new();
                }
                if let Some((expected, enable)) = self.voice.checking
                    && expected == id
                {
                    self.voice.checking = None;
                    let restored = self.voice.restoring == Some(id);
                    if restored {
                        self.voice.restoring = None;
                    }
                    self.checked_speech(enable, restored, result);
                }
            }
            Reply::Recording(id) => self.voice_reply_recording(id),
            Reply::Transcribing(id) => {
                self.voice_reply_transcribing(id);
                if let Some(capture) = &mut self.voice.capture
                    && capture.id == id
                    && !capture.cancelled
                {
                    capture.phase = "voice transcribing /voice cancel";
                }
            }
            Reply::Recorded { id, result } => {
                if self
                    .voice
                    .replies
                    .active
                    .as_ref()
                    .is_some_and(|c| c.id == id)
                {
                    return self.voice_reply_recorded(id, result);
                }
                self.recorded_reply(id, result);
            }
            Reply::Synthesized { id, result } => {
                let Some(speech) = &self.voice.speech else {
                    return Vec::new();
                };
                if speech.id != id {
                    return Vec::new();
                }
                if !speech.cancelled
                    && self.voice.enabled
                    && self.connected
                    && self.tick < speech.question.expires
                    && self.question_live(&speech.question)
                {
                    match result {
                        Ok((config, pcm, rate)) => {
                            if let Some(speech) = &mut self.voice.speech {
                                speech.playing = true;
                            }
                            return vec![Effect::Voice(VoiceEffect::Play {
                                id,
                                config,
                                pcm,
                                rate,
                            })];
                        }
                        Err(error) => {
                            self.reply_unavailable("speech_failed: synthesis failed; inspect TUI/configuration; answer in TUI");
                            self.flash_error(format!("question speech failed: {error}"));
                        }
                    }
                }
                if self
                    .voice
                    .speech
                    .as_ref()
                    .is_some_and(|s| s.cancelled || self.tick >= s.question.expires)
                {
                    self.reply_unavailable("speech_cancelled: playback did not complete; answer in TUI or wait for a new question");
                }
                self.voice.speech = None;
            }
            Reply::Played { id, result } => {
                if self.voice.speech.as_ref().is_some_and(|s| s.id == id) {
                    if self.voice.speech.as_ref().is_some_and(|s| s.cancelled) {
                        self.reply_unavailable("speech_cancelled: playback did not complete; answer in TUI or wait for a new question");
                    } else if result.is_err() {
                        self.reply_unavailable("playback_failed: playback did not complete; inspect TUI/audio output; answer in TUI");
                    }
                }
                self.arm_spoken_reply(id, &result);
                if let Some(speech) = &self.voice.speech
                    && speech.id == id
                {
                    if let Err(error) = result
                        && !speech.cancelled
                    {
                        self.flash_error(format!("question playback failed: {error}"));
                    }
                    self.voice.speech = None;
                }
            }
            Reply::SetupProgress { id, stage } => {
                if let Some(install) = &mut self.voice.install
                    && install.id == id
                {
                    install.stage = stage;
                }
            }
            Reply::SetupDone { id, result } => {
                let installed = self.setup_done(id, result);
                if installed {
                    // Installing a backend is not consent to speak: only a
                    // saved setting may come back, after the same validation.
                    self.rearm_saved_voice();
                }
            }
        }
        Vec::new()
    }

    /// One finished `/voice check`, `/voice on`, or restored preference. A late
    /// success must never revive a state a mute or disconnect already left.
    fn checked_speech(
        &mut self,
        enable: bool,
        restored: bool,
        result: Result<String, CheckFailure>,
    ) {
        let live = self.connected && !self.quit_requested;
        let error = match result {
            Ok(message) if enable && !live => {
                self.flash(format!(
                    "{message}; question speech left off: disconnected or quitting"
                ));
                return;
            }
            Ok(message) => {
                self.voice.enabled |= enable;
                if restored {
                    self.voice.restored = true;
                    self.flash(self.voice_key_hint(&format!(
                        "{message}; saved setting restored: questions asked from now on are spoken; nothing records until you press F4"
                    )));
                    return;
                }
                // A check that was only ever a diagnostic saves nothing.
                let saved = if enable {
                    self.persist_voice(VoicePrefs {
                        questions: true,
                        replies: self.voice.replies.enabled,
                    })
                } else {
                    String::new()
                };
                let state = if self.voice.enabled {
                    "on for new questions only"
                } else {
                    "off"
                };
                self.flash(format!("{message}; question speech {state}{saved}"));
                return;
            }
            Err(error) => error,
        };
        self.flash_error(if restored {
            format!(
                "saved voice settings not restored: {error}; /voice opens settings and installs what is missing"
            )
        } else if error.missing.is_some() && enable {
            format!("question speech stays off: {error}")
        } else {
            format!("voice configuration failed: {error}")
        });
    }

    /// Remember which installable backend a current check lacked, so the open
    /// `/voice` popup selects its install row. Nothing is installed or enabled.
    fn note_missing_backend(&mut self, id: u64, result: &Result<String, CheckFailure>) {
        let current = self.voice.replies.checking == Some(id)
            || self
                .voice
                .checking
                .is_some_and(|(expected, _)| expected == id);
        if !current {
            return;
        }
        assert!(
            id > self.voice.missing_serial(),
            "check ids only grow, so a current check is newer than the last failure"
        );
        self.voice.missing = match result {
            Ok(_) => None,
            Err(failure) => failure.missing.map(|missing| (id, missing)),
        };
    }

    fn recorded_reply(&mut self, id: u64, result: Result<String, String>) {
        if !self.voice.capture.as_ref().is_some_and(|c| c.id == id) {
            return;
        }
        let Some(capture) = self.voice.capture.take() else {
            return;
        };
        if capture.cancelled {
            self.flash(match result {
                Err(error) if error != "cancelled" => {
                    format!("voice cancelled; backend also failed: {error}")
                }
                _ => "voice recording cancelled; draft unchanged".into(),
            });
            return;
        }
        match result {
            Ok(text) => {
                let current = !crate::viewer::hides_composer(self)
                    && self.selected == Some(capture.agent)
                    && self.voice.selection == capture.selection
                    && self.input.revision == capture.revision
                    && self.input.text == capture.draft
                    && self.input.cursor == capture.cursor;
                if current {
                    self.input.insert_str(&text);
                    self.flash("voice draft inserted; review before sending");
                } else {
                    self.voice.recovered = Some((capture.agent, text));
                    self.flash(self.voice_key_hint("voice draft retained: composer/agent changed; select original agent and F5 (or /voice insert); /voice cancel discards"));
                }
            }
            Err(error) => self.flash_error(format!(
                "local transcription failed: {error}; draft unchanged"
            )),
        }
    }
}
