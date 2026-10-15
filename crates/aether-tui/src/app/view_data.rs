//! Data the slash views show, fetched by the reducer and read at draw time.
use std::collections::HashSet;

use aether_core::artifact::Artifact;
use aether_core::id::{AgentId, ArtifactId, RunId, ScheduleId};
use aether_core::report::RunReport;
use aether_core::schedule::{Schedule, ScheduleOccurrence};

#[derive(Debug, Default)]
pub struct ScheduleView {
    pub jobs: Vec<Schedule>,
    /// Missing names already looked up; explicit refresh can still resolve them.
    pub(super) name_lookups: HashSet<ScheduleId>,
    pub history: Option<(ScheduleId, Vec<ScheduleOccurrence>)>,
    pub history_scroll: u16,
    pub loading: bool,
    pub refresh_pending: bool,
    pub error: Option<String>,
    pub confirm_cancel: Option<ScheduleId>,
}

/// `/report`'s fetched document: which run it is for, and what came back.
#[derive(Debug, Default)]
pub struct ReportView {
    pub run: Option<RunId>,
    pub report: Option<Box<RunReport>>,
    pub loading: bool,
    pub error: Option<String>,
}

/// `/artifacts` and its pager.
#[derive(Debug, Default)]
pub struct ArtifactsState {
    /// Whose list `list` is; a reply for anyone else is stale.
    pub for_agent: Option<AgentId>,
    pub list: Vec<Artifact>,
    /// Followed from a link before the list arrived: the cursor lands on it
    /// once `Then::Artifacts` answers, not against the empty list.
    /// The artifact whose bytes were asked for. A reply naming anything else
    /// is stale and must not reopen a pager the person has closed.
    /// Paired with the read's number, so an earlier read's reply never
    /// settles a pager reopened since.
    pub pending_bytes: Option<(ArtifactId, u64)>,
    /// Bytes of the artifact the pager shows.
    pub open: Option<OpenArtifact>,
}

/// An artifact's bytes, decoded once when they arrive rather than on every
/// frame. Bytes that are not text are described, never painted as mojibake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenArtifact {
    pub artifact: Artifact,
    pub text: String,
    /// The daemon sent only the head of the payload.
    pub truncated: bool,
}
