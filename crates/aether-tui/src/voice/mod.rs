//! Optional client-owned local audio. No daemon messages carry microphone audio.

mod backend;
mod config;
pub mod control;
mod piper;
mod process;
mod service;
pub(crate) mod setup;
mod sherpa;
#[cfg(test)]
pub(crate) mod test_script;
#[cfg(test)]
mod tests;
pub(crate) mod ui;

pub use config::{Config, Missing, SttBackend};
pub(crate) use service::Service;
pub(crate) use setup::ModelKind;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Check,
    On,
    Off,
    Mute,
    RepliesOn,
    RepliesOff,
    Start,
    Stop,
    Cancel,
    Insert,
    /// Open the guided voice setup popup.
    Setup,
}

impl Action {
    pub fn parse(arg: &str) -> Option<Self> {
        Some(match arg {
            "status" => Self::Status,
            "check" | "reload" => Self::Check,
            "on" => Self::On,
            "off" => Self::Off,
            "mute" => Self::Mute,
            "replies on" => Self::RepliesOn,
            "replies off" => Self::RepliesOff,
            "start" => Self::Start,
            "stop" => Self::Stop,
            "cancel" => Self::Cancel,
            "insert" => Self::Insert,
            "" | "setup" => Self::Setup,
            _ => return None,
        })
    }
}

// Moved once through the runner channel; keep payload ownership direct.
#[allow(clippy::large_enum_variant)]
#[derive(PartialEq)]
pub enum Effect {
    Check {
        id: u64,
        dir: PathBuf,
        speech: bool,
    },
    CheckReplies {
        id: u64,
        dir: PathBuf,
    },
    RecordReply {
        id: u64,
        dir: PathBuf,
    },
    Record {
        id: u64,
        dir: PathBuf,
    },
    Stop(u64),
    Cancel(u64),
    Synthesize {
        id: u64,
        dir: PathBuf,
        text: String,
    },
    Play {
        id: u64,
        config: Config,
        pcm: Vec<u8>,
        rate: u32,
    },
    /// Install a pinned whisper.cpp backend/model into a user-owned managed
    /// directory and update `voice.toml`. Runs entirely offline of any
    /// agent/daemon connection.
    Setup {
        id: u64,
        dir: PathBuf,
        model: setup::ModelKind,
    },
}

// Moved once through the runner channel; keep payload ownership direct.
#[allow(clippy::large_enum_variant)]
pub enum Reply {
    Checked {
        id: u64,
        result: Result<String, CheckFailure>,
    },
    /// First microphone PCM bytes received, not merely a spawned recorder.
    Recording(u64),
    Transcribing(u64),
    Recorded {
        id: u64,
        result: Result<String, String>,
    },
    Synthesized {
        id: u64,
        result: Result<(Config, Vec<u8>, u32), String>,
    },
    Played {
        id: u64,
        result: Result<(), String>,
    },
    /// One human-readable install stage, e.g. "cloning whisper.cpp v1.9.4".
    SetupProgress {
        id: u64,
        stage: String,
    },
    SetupDone {
        id: u64,
        result: Result<String, String>,
    },
}

/// Why a configuration check failed, and which installable backend it lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckFailure {
    pub message: String,
    pub missing: Option<Missing>,
}

impl From<&str> for CheckFailure {
    fn from(message: &str) -> Self {
        CheckFailure {
            message: message.to_owned(),
            missing: None,
        }
    }
}

impl From<Missing> for CheckFailure {
    fn from(missing: Missing) -> Self {
        CheckFailure {
            message: missing.to_string(),
            missing: Some(missing),
        }
    }
}

impl std::fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, id) = match self {
            Self::Check { id, .. } => ("Check", id),
            Self::Record { id, .. } => ("Record", id),
            Self::RecordReply { id, .. } => ("RecordReply", id),
            Self::CheckReplies { id, .. } => ("CheckReplies", id),
            Self::Stop(id) => ("Stop", id),
            Self::Cancel(id) => ("Cancel", id),
            Self::Synthesize { id, .. } => ("Synthesize", id),
            Self::Play { id, .. } => ("Play", id),
            Self::Setup { id, .. } => ("Setup", id),
        };
        f.debug_tuple(kind).field(id).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, id) = match self {
            Self::Checked { id, .. } => ("Checked", id),
            Self::Recording(id) => ("Recording", id),
            Self::Transcribing(id) => ("Transcribing", id),
            Self::Recorded { id, .. } => ("Recorded", id),
            Self::Synthesized { id, .. } => ("Synthesized", id),
            Self::Played { id, .. } => ("Played", id),
            Self::SetupProgress { id, .. } => ("SetupProgress", id),
            Self::SetupDone { id, .. } => ("SetupDone", id),
        };
        f.debug_tuple(kind).field(id).finish_non_exhaustive()
    }
}
