use std::{path::PathBuf, sync::Mutex};

use crate::{EngineKind, SessionRecorder};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RecordedSession {
    pub engine: EngineKind,
    pub id: String,
    pub log_paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub(crate) struct Recorder {
    pub records: Mutex<Vec<RecordedSession>>,
    pub fail: bool,
}

impl SessionRecorder for Recorder {
    fn record(&self, engine: EngineKind, id: &str, log_paths: Vec<PathBuf>) -> Result<(), String> {
        if self.fail {
            return Err("test ownership storage unavailable".into());
        }
        self.records.lock().unwrap().push(RecordedSession {
            engine,
            id: id.into(),
            log_paths,
        });
        Ok(())
    }
}
