//! What the TUI remembers between runs.
//!
//! The TUI owns `<config dir>/ui.toml` outright: `config.toml` is hand-written
//! and keeps its comments, so nothing here ever rewrites it. A command that
//! changes a preference saves the whole file; a `--flag` overrides for the
//! session without saving.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ui::theme::ThemeChoice;

pub const FILE: &str = "ui.toml";

/// How many recent models the picker orders by. Enough to cover a session's
/// switching without the list becoming the whole registry again.
const RECENT_MODELS: usize = 8;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// How many recent workspaces the picker lists.
pub const RECENT_WORKSPACES: usize = 12;

const HEADER: &str = "# Aether TUI preferences. The app rewrites this file, so keep comments\n\
                      # elsewhere. `theme` names a theme (auto, dark, light, mono, nord, ... or\n\
                      # a file in themes/); /theme changes it live.\n\
                      # `ascii = true` uses ASCII decoration, not ASCII message text.\n";

/// Preferences as stored. Absent fields keep their defaults, so an older
/// file still loads after a new preference is added; an unknown field is
/// tolerated on load rather than rejected, so a file a newer build wrote
/// still loads here, but this build's own save then drops it - there is no
/// catch-all, so a mixed-version round trip loses whatever field this
/// build does not know about.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub view_mode: Option<String>,
    pub last_seq: Option<u64>,
    pub draft: Option<String>,
    /// Raw overrides keep bad entries visible on startup and across unrelated saves.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub keys: std::collections::BTreeMap<String, toml::Value>,
    /// Absent is `auto`. Files written before themes say `dusk`/`dawn`,
    /// read as dark/light; an unreadable name is an error, not a default.
    pub theme: Option<ThemeChoice>,
    /// A still spinner and instant scrolling (VISION §9.7).
    pub reduced_motion: Option<bool>,
    /// ASCII decoration in the interactive UI; content is unchanged.
    pub ascii: Option<bool>,
    /// Whether new agents auto-approve. Absent leaves it to the daemon's
    /// `[approval] default`.
    pub yolo: Option<bool>,
    /// The session default model, as `provider/model[,fallback...]`. Kept as
    /// the written form so an unparseable value is a clear error, and used
    /// for every new agent the client starts.
    pub model: Option<String>,
    /// Models chosen recently, newest first: what orders the `/model` picker.
    pub recent_models: Vec<String>,
    /// Show work cleared out of the view. Absent hides it, which is the
    /// point of `/clear`.
    pub show_archived: Option<bool>,
    /// Speak agent questions aloud again on the next launch. Absent is off; a
    /// saved `true` is an opt-in the TUI still re-validates before honouring.
    pub voice_questions: Option<bool>,
    /// Arm global voice replies again on the next launch. Implies
    /// `voice_questions`: a reply is only ever offered for a spoken question.
    pub voice_replies: Option<bool>,
    /// Ctrl+B board open on attach; only restored when wide enough to dock.
    pub board_open: Option<bool>,
    pub mouse: Option<MouseMode>,
    /// Tab-queued messages per agent id, restored on attach and never auto-sent.
    pub queued: std::collections::BTreeMap<String, Vec<String>>,
    /// Directories agents were started in, newest first: the picker's recents.
    pub recent_workspaces: Vec<String>,
    /// Workspaces the picker lists first, in the order they were pinned.
    pub pinned_workspaces: Vec<String>,
    /// Where the picker looks for git projects, `~` allowed. Absent is home;
    /// an empty list looks nowhere. Written by hand, never by the app.
    pub workspace_roots: Option<Vec<String>>,
    /// Directory levels below each root the picker searches; absent is 2.
    pub workspace_depth: Option<usize>,
}

/// `/settings → mouse` (SPEC §4.3). Capture is on unless explicitly disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseMode {
    Off,
    /// Capture wheel, clicks and selection; Shift-drag uses terminal selection.
    #[default]
    Scroll,
    /// Legacy spelling of capture, with the same behaviour as `Scroll`.
    Full,
}

/// The voice settings that survive a restart. Both off is the default and
/// the only state reached without an explicit opt-in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VoicePrefs {
    /// Speak newly asked agent questions aloud.
    pub questions: bool,
    /// Global shortcut records a reply and sends it without review.
    pub replies: bool,
}

impl VoicePrefs {
    /// Whether anything at all was opted into; both off needs no restoring.
    pub fn any(self) -> bool {
        debug_assert!(
            self.questions || !self.replies,
            "replies without question speech is not a representable state"
        );
        self.questions || self.replies
    }
}

impl UiPrefs {
    /// Serialize read-modify-write updates across TUI processes.
    pub(crate) fn update(dir: &Path, change: impl FnOnce(&mut Self)) -> Result<(), PrefsError> {
        let path = dir.join("ui.lock");
        aether_model::config::create_config_dir(dir)
            .map_err(|e| PrefsError::new(&path, e.to_string()))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| PrefsError::new(&path, e.to_string()))?;
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(PrefsError::new(
                            &path,
                            "preferences are busy in another TUI; retry after it resumes or exits"
                                .into(),
                        ));
                    }
                    std::thread::sleep(remaining.min(LOCK_RETRY));
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(PrefsError::new(&path, error.to_string()));
                }
            }
        }
        let mut prefs = Self::load(dir)?;
        change(&mut prefs);
        prefs.save(dir)
    }

    /// Read `ui.toml`. A missing file is no preferences; an unreadable or
    /// malformed one is an error the caller shows rather than silently losing.
    pub fn load(dir: &Path) -> Result<UiPrefs, PrefsError> {
        let path = dir.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| PrefsError::new(&path, e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UiPrefs::default()),
            Err(e) => Err(PrefsError::new(&path, e.to_string())),
        }
    }

    /// Write the whole file, atomically: an interrupted save must never leave
    /// a half-written `ui.toml` that fails every later `load`.
    pub fn save(&self, dir: &Path) -> Result<(), PrefsError> {
        let path = dir.join(FILE);
        let body =
            toml::to_string_pretty(self).map_err(|e| PrefsError::new(&path, e.to_string()))?;
        aether_model::config::create_config_dir(dir)
            .map_err(|e| PrefsError::new(&path, e.to_string()))?;
        // Same directory, so the rename is on one filesystem and is atomic.
        let temp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
        let write = std::fs::write(&temp, format!("{HEADER}{body}"))
            .and_then(|()| std::fs::rename(&temp, &path));
        if let Err(e) = write {
            let _ = std::fs::remove_file(&temp);
            return Err(PrefsError::new(&path, e.to_string()));
        }
        Ok(())
    }

    /// Whether to move at all: an explicit flag beats the environment, which
    /// beats the saved preference, which defaults to full motion.
    pub fn reduced_motion(&self, flag: bool) -> bool {
        flag || crate::motion::Motion::env_reduced()
            .or(self.reduced_motion)
            .unwrap_or(false)
    }

    /// An explicit flag beats the environment, then the saved preference.
    pub fn ascii(&self, flag: bool, environment: Option<&str>) -> Result<bool, String> {
        let environment = match environment.map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(raw) => Some(match raw.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => return Err("AETHER_ASCII must be true/false, yes/no, on/off or 1/0".into()),
            }),
        };
        Ok(flag || environment.or(self.ascii).unwrap_or(false))
    }

    /// The session default chain, when one was saved and still parses. An
    /// unreadable value is dropped rather than failing the whole file: the
    /// registry may simply no longer carry that model.
    pub fn model_chain(&self) -> Option<aether_core::model::ModelChain> {
        self.model
            .as_deref()
            .and_then(aether_core::model::ModelChain::parse)
    }

    /// Remember `chain` as the newest choice, newest first, without repeats.
    pub fn remember_model(&mut self, chain: &aether_core::model::ModelChain) {
        let name = chain.primary.to_string();
        self.recent_models.retain(|m| *m != name);
        self.recent_models.insert(0, name);
        self.recent_models.truncate(RECENT_MODELS);
    }

    /// Remember `path` as the newest workspace, newest first, without repeats.
    /// The caller passes the canonical path, so one directory is one entry.
    pub fn remember_workspace(&mut self, path: &str) {
        let name = path.to_owned();
        self.recent_workspaces.retain(|w| *w != name);
        self.recent_workspaces.insert(0, name);
        self.recent_workspaces.truncate(RECENT_WORKSPACES);
    }

    /// The saved voice settings, normalised so replies always imply question
    /// speech; a hand-edited contradiction is read the safer way round.
    pub fn voice(&self) -> VoicePrefs {
        let replies = self.voice_replies.unwrap_or(false);
        VoicePrefs {
            questions: replies || self.voice_questions.unwrap_or(false),
            replies,
        }
    }

    /// Record what the next launch should restore. Written as explicit
    /// `false` rather than absence so an opt-out is a decision, not a gap.
    pub fn set_voice(&mut self, voice: VoicePrefs) {
        assert!(
            voice.questions || !voice.replies,
            "replies without question speech must never be saved"
        );
        self.voice_questions = Some(voice.questions);
        self.voice_replies = Some(voice.replies);
    }

    /// The theme to paint with: `--theme` beats a non-empty `NO_COLOR`, which
    /// beats the saved theme, then `auto`.
    pub fn theme(&self, flag: Option<ThemeChoice>, no_color: Option<&str>) -> ThemeChoice {
        let no_color = no_color_set(no_color).then(ThemeChoice::mono);
        flag.or(no_color)
            .or_else(|| self.theme.clone())
            .unwrap_or_default()
    }
}

/// Whether `NO_COLOR` is what picked the theme: set and not outranked by `--theme`.
pub fn no_color_picked_theme(flagged: bool, no_color: Option<&str>) -> bool {
    !flagged && no_color_set(no_color)
}

/// An empty `NO_COLOR` is unset (no-color.org).
fn no_color_set(no_color: Option<&str>) -> bool {
    no_color.is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct PrefsError {
    pub path: String,
    pub reason: String,
}

impl PrefsError {
    fn new(path: &Path, reason: String) -> PrefsError {
        PrefsError {
            path: path.display().to_string(),
            reason,
        }
    }
}

/// Keep local preferences in their legacy location; isolate remote session state.
pub fn session_dir(endpoint: &aether_client::Endpoint, instance: &str, shared: &Path) -> PathBuf {
    if endpoint.is_remote() {
        endpoint.state_dir(instance)
    } else {
        shared.to_owned()
    }
}

/// Shared configuration, custom themes and modes.
pub fn dir() -> PathBuf {
    aether_model::config::config_dir(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            UiPrefs::load(dir.path()).expect("missing"),
            UiPrefs::default()
        );

        let prefs = UiPrefs {
            view_mode: Some("focus".into()),
            last_seq: Some(12),
            draft: Some("later".into()),
            theme: Some("nord-light".parse().unwrap()),
            reduced_motion: Some(true),
            ascii: Some(true),
            yolo: Some(false),
            model: Some("mock/m".into()),
            recent_models: vec!["mock/m".into()],
            show_archived: Some(true),
            voice_questions: Some(true),
            voice_replies: Some(false),
            board_open: Some(true),
            mouse: Some(MouseMode::Scroll),
            queued: [("agt_1".to_owned(), vec!["later".to_owned()])].into(),
            recent_workspaces: vec!["/work/aether".into()],
            pinned_workspaces: vec!["/work/murals".into(), "~/notes".into()],
            workspace_roots: Some(vec!["~/work".into(), "/srv/src".into()]),
            workspace_depth: Some(3),
            keys: [("details".into(), "f2".into())].into(),
        };
        prefs.save(dir.path()).expect("save");
        assert_eq!(UiPrefs::load(dir.path()).expect("load"), prefs);
    }

    #[test]
    fn old_theme_names_still_load_and_new_ones_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (written, saved) in [
            ("dusk", "dark"),
            ("dawn", "light"),
            ("auto", "auto"),
            ("dark", "dark"),
            ("light", "light"),
            ("mono", "mono"),
            ("nord", "nord"),
            ("catppuccin-latte", "catppuccin-light"),
            ("my-theme", "my-theme"),
        ] {
            std::fs::write(dir.path().join(FILE), format!("theme = \"{written}\"\n"))
                .expect("write");
            let prefs = UiPrefs::load(dir.path()).expect("load");
            let theme = prefs.theme.clone().expect("a theme");
            assert_eq!(theme.to_string(), saved, "{written}");
            prefs.save(dir.path()).expect("save");
            let text = std::fs::read_to_string(dir.path().join(FILE)).expect("read");
            assert!(text.contains(&format!("theme = \"{saved}\"")), "{text}");
            assert_eq!(
                UiPrefs::load(dir.path()).expect("reload").theme,
                Some(theme)
            );
        }
    }

    #[test]
    fn the_theme_ranks_flag_then_no_color_then_saved_then_auto() {
        let parse = |name: &str| name.parse::<ThemeChoice>().expect(name);
        let saved = UiPrefs {
            theme: Some(parse("gruvbox")),
            ..Default::default()
        };
        let name = |prefs: &UiPrefs, flag: Option<&str>, no_color| {
            prefs.theme(flag.map(parse), no_color).to_string()
        };
        assert_eq!(name(&UiPrefs::default(), None, None), "auto");
        assert_eq!(name(&saved, None, None), "gruvbox");
        assert_eq!(name(&saved, None, Some("1")), "mono");
        // An empty NO_COLOR is unset (no-color.org).
        assert_eq!(name(&saved, None, Some("")), "gruvbox");
        assert_eq!(name(&saved, Some("nord-dark"), Some("1")), "nord-dark");
    }

    #[test]
    fn no_color_picks_the_theme_only_when_set_and_not_outranked_by_the_flag() {
        assert!(no_color_picked_theme(false, Some("1")));
        assert!(!no_color_picked_theme(false, Some("")));
        assert!(!no_color_picked_theme(false, None));
        assert!(!no_color_picked_theme(true, Some("1")));
    }

    #[test]
    fn a_save_leaves_no_temp_file_and_a_newer_file_still_loads() {
        let dir = tempfile::tempdir().expect("tempdir");
        UiPrefs {
            theme: Some(ThemeChoice::mono()),
            ..Default::default()
        }
        .save(dir.path())
        .expect("save");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert_eq!(left, vec![FILE.to_owned()], "{left:?}");

        // A preference a later build added must not lock this build out.
        std::fs::write(
            dir.path().join(FILE),
            "theme = \"mono\"\nsomething_new = 3\n",
        )
        .expect("write");
        assert_eq!(
            UiPrefs::load(dir.path()).expect("load").theme,
            Some(ThemeChoice::mono())
        );
    }

    #[test]
    fn voice_defaults_to_off_and_replies_always_imply_question_speech() {
        assert_eq!(UiPrefs::default().voice(), VoicePrefs::default());
        assert!(!UiPrefs::default().voice().any());

        // A file from before these fields existed must still mean off.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE), "theme = \"dusk\"\n").expect("write");
        assert_eq!(
            UiPrefs::load(dir.path()).expect("load").voice(),
            VoicePrefs::default()
        );

        // A hand-edited contradiction is read the safer way round.
        let contradictory = UiPrefs {
            voice_questions: Some(false),
            voice_replies: Some(true),
            ..Default::default()
        };
        assert_eq!(
            contradictory.voice(),
            VoicePrefs {
                questions: true,
                replies: true
            }
        );

        let mut prefs = UiPrefs::default();
        prefs.set_voice(VoicePrefs {
            questions: true,
            replies: true,
        });
        prefs.save(dir.path()).expect("save");
        let loaded = UiPrefs::load(dir.path()).expect("load");
        assert_eq!(loaded.voice(), prefs.voice());
        assert!(loaded.voice().any());

        // Opting out is stored as a decision, not as an absent field.
        prefs.set_voice(VoicePrefs::default());
        assert_eq!(prefs.voice_questions, Some(false));
        assert_eq!(prefs.voice_replies, Some(false));
    }

    #[test]
    fn recent_workspaces_are_newest_first_unique_and_capped() {
        let mut prefs = UiPrefs::default();
        for n in 0..15 {
            prefs.remember_workspace(&format!("/w/{n}"));
        }
        prefs.remember_workspace("/w/5");
        assert_eq!(prefs.recent_workspaces.len(), RECENT_WORKSPACES);
        assert_eq!(prefs.recent_workspaces[0], "/w/5");
        assert_eq!(prefs.recent_workspaces[1], "/w/14");
        assert_eq!(
            prefs
                .recent_workspaces
                .iter()
                .filter(|w| *w == "/w/5")
                .count(),
            1
        );
        assert!(!prefs.recent_workspaces.contains(&"/w/0".to_owned()));

        let dir = tempfile::tempdir().expect("tempdir");
        prefs.save(dir.path()).expect("save");
        assert_eq!(
            UiPrefs::load(dir.path()).expect("load").recent_workspaces,
            prefs.recent_workspaces
        );
    }

    #[test]
    fn workspace_discovery_and_pins_default_to_home_and_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE), "theme = \"mono\"\n").expect("write");
        let prefs = UiPrefs::load(dir.path()).expect("load");
        assert_eq!(prefs.workspace_roots, None, "absent is home");
        assert_eq!(prefs.workspace_depth, None);
        assert!(prefs.pinned_workspaces.is_empty());
        prefs.save(dir.path()).expect("save");
        let text = std::fs::read_to_string(dir.path().join(FILE)).expect("read");
        assert!(!text.contains("workspace_roots"), "{text}");
        assert!(!text.contains("workspace_depth"), "{text}");

        // An empty list is a choice to look nowhere, kept apart from absent.
        std::fs::write(
            dir.path().join(FILE),
            "workspace_roots = []\nworkspace_depth = 3\npinned_workspaces = [\"~/a\"]\n",
        )
        .expect("write");
        let prefs = UiPrefs::load(dir.path()).expect("load");
        assert_eq!(prefs.workspace_roots, Some(Vec::new()));
        prefs.save(dir.path()).expect("save");
        assert_eq!(UiPrefs::load(dir.path()).expect("reload"), prefs);
        assert_eq!(prefs.workspace_depth, Some(3));
        assert_eq!(prefs.pinned_workspaces, ["~/a"]);
    }

    #[test]
    fn a_bad_file_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(FILE), "theme = \"../neon\"\n").expect("write");
        assert!(UiPrefs::load(dir.path()).is_err());
    }

    #[test]
    fn preference_update_locks_the_entire_read_modify_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        UiPrefs::update(dir.path(), |prefs| {
            let contender = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.path().join("ui.lock"))
                .expect("lock file");
            assert!(matches!(
                contender.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            prefs.ascii = Some(true);
        })
        .expect("update");
        UiPrefs::update(dir.path(), |prefs| {
            assert_eq!(prefs.ascii, Some(true));
            prefs.reduced_motion = Some(true);
        })
        .expect("second update");
        let saved = UiPrefs::load(dir.path()).expect("saved preferences");
        assert_eq!(saved.ascii, Some(true));
        assert_eq!(saved.reduced_motion, Some(true));
    }

    #[test]
    fn preference_update_times_out_without_editing_and_can_retry_after_unlock() {
        let dir = tempfile::tempdir().expect("private fixture");
        let saved = UiPrefs {
            reduced_motion: Some(true),
            ..Default::default()
        };
        saved.save(dir.path()).expect("saved fixture");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join("ui.lock"))
            .expect("peer lock");
        lock.lock().expect("peer holds the lock");
        let path = dir.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = UiPrefs::update(&path, |prefs| prefs.ascii = Some(true));
            tx.send(result).expect("report update result");
        });
        let result = rx.recv_timeout(Duration::from_secs(5));
        drop(lock);
        worker.join().expect("update worker");
        let error = result
            .expect("a held lock must not wait indefinitely")
            .expect_err("a busy peer must prevent the update");
        assert!(error.to_string().contains("busy in another TUI"));
        assert_eq!(UiPrefs::load(dir.path()).expect("unchanged file"), saved);
        UiPrefs::update(dir.path(), |prefs| prefs.ascii = Some(true))
            .expect("retry after peer releases the lock");
        let updated = UiPrefs::load(dir.path()).expect("updated file");
        assert_eq!(updated.ascii, Some(true));
        assert_eq!(updated.reduced_motion, saved.reduced_motion);
    }

    #[test]
    fn preference_update_reports_lock_and_load_failures_without_overwriting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join(FILE);
        std::fs::write(&file, "invalid = [").expect("bad TOML");
        assert!(
            UiPrefs::update(dir.path(), |_| panic!("invalid file must not be edited")).is_err()
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "invalid = [");
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::create_dir(blocked.join("ui.lock")).unwrap();
        assert!(UiPrefs::update(&blocked, |_| panic!("no lock must mean no edit")).is_err());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod upgrade_tests {
    use super::*;

    #[test]
    fn local_upgrade_changes_one_preference_and_preserves_the_complete_legacy_file() {
        let shared = tempfile::tempdir().expect("config");
        let endpoint = aether_client::Endpoint::Socket(shared.path().join("daemon.sock"));
        let mut original = UiPrefs {
            model: Some("mock/m".into()),
            keys: [("details".into(), "f2".into())].into(),
            theme: Some("nord".parse().expect("theme")),
            pinned_workspaces: vec!["/work/project".into()],
            queued: [("agt_1".into(), vec!["pending".into()])].into(),
            ..Default::default()
        };
        original.save(shared.path()).expect("legacy preferences");
        let directory = session_dir(&endpoint, "new-store", shared.path());
        assert_eq!(directory, shared.path());
        assert_eq!(UiPrefs::load(&directory).expect("startup"), original);
        UiPrefs::update(&directory, |prefs| prefs.reduced_motion = Some(true)).expect("change");
        original.reduced_motion = Some(true);
        let restarted = session_dir(&endpoint, "new-store", shared.path());
        assert_eq!(UiPrefs::load(&restarted).expect("restart"), original);
        assert!(!shared.path().join("endpoints").exists());
    }
}
