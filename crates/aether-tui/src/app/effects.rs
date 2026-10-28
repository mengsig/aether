//! Application effects.
use super::{
    AgentId, ApprovalId, ArtifactId, ClientError, Command, KeyEvent, Reply, StartId,
    SubscriptionFrame,
};

/// What a memory curation keystroke asked for, so the notice names the action
/// rather than guessing it from the record that comes back - a restored
/// record is neither forgotten nor pinned, and read that way it says
/// "unpinned".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAction {
    Pinned,
    Unpinned,
    Forgotten,
    Restored,
}

impl std::fmt::Display for MemoryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pinned => "pinned",
            Self::Unpinned => "unpinned",
            Self::Forgotten => "forgot",
            Self::Restored => "restored",
        })
    }
}

/// What handler a reply should go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Host(super::host::HostRequest),
    RemoteAck {
        key: String,
        next: Box<Then>,
    },
    VoiceReply(u64),
    /// The daemon's defaults, fetched over `link` to start this root.
    RootDefaults {
        start: StartId,
        link: u64,
    },
    Ignore,
    MessageSent {
        agent: AgentId,
        message: super::images::Outgoing,
    },
    CompactionAccepted(AgentId),
    Status,
    /// The periodic refresh: only its own answer frees the next poll.
    StatusPoll,
    Tree,
    /// A page of an agent's history, asked for on `connection`.
    Backfill {
        agent: AgentId,
        connection: u64,
    },
    /// Reply to this root start's create request.
    AgentCreated(StartId),
    /// Reply to a restart: select the new agent and load what it inherited.
    Restarted {
        from: AgentId,
    },
    /// Reply to a wake: the same agent is live again; refresh its row.
    Woken,
    /// Reply to a fanned-out message: say how many agents it reached.
    FannedOut,
    Approvals {
        revision: u64,
    },
    ApprovalRules,
    McpServers,
    /// Reply to a memory browser refresh.
    Memories,
    /// Reply to a pin/unpin, a forget or a restore; the list refreshes once it
    /// lands. Carries what was asked for: the record that comes back cannot
    /// say which of them it was.
    MemoryMutated(MemoryAction),
    Models,
    /// The notification center's list of open ones, asked for on connect.
    Notifications,
    /// Its resolved history, asked for once the open ones are in.
    ResolvedNotifications,
    /// Reply to marking a notification read or resolving it.
    NotificationChanged,
    /// Reply to answering a notification.
    NotificationAnswered,
    Providers,
    Schedules,
    ScheduleChanged,
    ScheduleHistory(aether_core::id::ScheduleId),
    Usage,
    /// Reply to `/report`'s fetch: which run it was asked about.
    RunReport(aether_core::id::RunId),
    Invocations(AgentId),
    Artifacts(AgentId),
    /// An artifact's bytes, for the pager's read numbered `read`.
    ArtifactBytes {
        id: ArtifactId,
        read: u64,
    },
    AgentPatched,
    ModelPatched(AgentId),
    PendingRootModelPatched(AgentId),
    AgentRenamed(AgentId),
    MemoryCreated {
        selected: Option<AgentId>,
    },
    Auth,
    /// Reply to an interrupt request for this agent.
    Interrupted(AgentId),
    /// Reply to a pause request rooted at this agent.
    Paused(AgentId),
    /// Reply to a resume request rooted at this agent.
    Resumed(AgentId),
    /// Reply to an answer for this approval; the card lives until it lands.
    ApprovalResolved(ApprovalId),
    /// Reply to a revoke; the rule is reported gone only once it is.
    RuleRevoked,
    /// Reply to an approvals change rooted at this agent; `on` is what the
    /// caller asked for, so the session default is saved only once the
    /// daemon actually agreed to it.
    ApprovalsSet {
        agent: AgentId,
        on: bool,
    },
    /// Reply to an archive request: how much left the view.
    Archived,
    BoardConfirm {
        agent: AgentId,
        remove: bool,
        connection: u64,
    },
    BoardStopped {
        agent: AgentId,
        remove: bool,
    },
    BoardArchived {
        agent: AgentId,
        archived: bool,
        after_stop: bool,
    },
    /// Reply to sending this agent's oldest queued message.
    QueuedSent(AgentId),
}

/// Side effects the loop executes on the reducer's behalf.
// Variants differ in size, but effects are built, moved once, and consumed.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq)]
pub enum Effect {
    PreviewFile(crate::viewer::ReadRequest),
    OpenFile(std::path::PathBuf),
    ReloadViewModes(std::path::PathBuf),
    Voice(crate::voice::Effect),
    Request {
        command: Command,
        then: Then,
    },
    Subscribe {
        after: Option<u64>,
    },
    /// Read the remote operations the host could not account for, after
    /// carrying out a person's decision about one of them when there is one.
    Outbox {
        settle: Option<(String, aether_client::Settlement)>,
    },
    /// Leave the terminal to `aether <args>` (login flows), then come back.
    RunExternal {
        args: Vec<String>,
    },
    #[cfg(unix)]
    Suspend,
    /// Turn terminal mouse capture on or off.
    SetMouse(bool),
    /// Put text on the system clipboard.
    Copy(String),
    /// Read an image for the draft; it comes back as `AppEvent::ImageRead`.
    AttachImage(crate::image_input::ImageSource),
    /// Read clipboard types, then an image or text, off the UI loop.
    PasteClipboard,
    /// Ring the terminal bell (attention while the window is unfocused).
    Bell,
    /// Ask the terminal to raise a desktop notification. Terminals that do
    /// not understand the escape simply ignore it.
    Notify {
        title: String,
        body: String,
    },
    Quit,
}

// Replies are large; events are moved once into the reducer and dropped.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AppEvent {
    PreviewReady {
        id: u64,
        result: Result<crate::viewer::Preview, String>,
    },
    ViewModesLoaded(crate::view_modes::Catalog),
    Voice(crate::voice::Reply),
    Key(KeyEvent),
    Paste(String),
    /// An empty bracketed paste delivered by the terminal.
    TerminalImagePaste,
    /// A paste that was exactly the path of an image file, and the file as read.
    ImagePathPasted {
        text: String,
        image: Result<aether_core::attachment::ImageAttachment, String>,
    },
    /// `Effect::AttachImage` finished; the error is one line for the status row.
    ImageRead(Result<aether_core::attachment::ImageAttachment, String>),
    ClipboardRead(Result<crate::image_input::ClipboardContent, String>),
    Resize(u16, u16),
    /// The terminal window gained or lost focus.
    Focus(bool),
    /// `frames` frames of the motion clock elapsed. The runner sends one per
    /// frame while something animates and one large batch when it wakes from
    /// idle, so deadlines stay right in wall time without a timer running.
    Tick {
        frames: u64,
    },
    Connected,
    HistoryExpired,
    /// The connection to the daemon ended. `because` is what the daemon said
    /// before closing, when it said anything - a tokened transport explains an
    /// idle drop, and reporting only "lost the daemon" throws that away.
    Disconnected {
        because: Option<String>,
    },
    /// An external command run through `Effect::RunExternal` finished.
    ExternalFinished {
        label: String,
        error: Option<String>,
    },
    Copied {
        lines: usize,
        confirmed: bool,
    },
    /// `Effect::Copy` or `Effect::SetMouse` failed. `action` names what was
    /// attempted, `error` is the raw cause - mirrors `ExternalFinished`,
    /// where the app builds the message rather than the runner.
    EffectFailed {
        action: String,
        error: String,
    },
    Subscribed {
        id: u64,
        last_seq: u64,
    },
    Frame(SubscriptionFrame),
    /// `Effect::Outbox` finished: how the decision went, and what still waits.
    Outbox {
        settled: Option<(
            aether_client::Settlement,
            Result<Option<Reply>, ClientError>,
        )>,
        undecided: Result<aether_client::Undecided, ClientError>,
    },
    Reply {
        then: Then,
        result: Result<Reply, ClientError>,
    },
}

impl Then {
    /// Reads every connection makes again: the connect snapshot asks for them
    /// and the next poll re-reads status, so an older answer only rolls back.
    pub(crate) fn superseded_on_reconnect(&self) -> bool {
        matches!(
            self,
            Then::Status
                | Then::StatusPoll
                | Then::Tree
                | Then::Approvals { .. }
                | Then::Models
                | Then::Auth
                | Then::Notifications
                | Then::ResolvedNotifications
        )
    }
}

impl AppEvent {
    /// Whether a protocol event still applies once the connection it came on
    /// was replaced. Outcomes of what the person asked for always do.
    pub(crate) fn outlives_connection(&self) -> bool {
        match self {
            AppEvent::Reply { then, .. } => !then.superseded_on_reconnect(),
            AppEvent::Outbox { .. } => true,
            // The new subscription replays from the cursor; the old stream's end is old news.
            AppEvent::Subscribed { .. }
            | AppEvent::Frame(_)
            | AppEvent::Disconnected { .. }
            | AppEvent::HistoryExpired => false,
            _ => true,
        }
    }
}
