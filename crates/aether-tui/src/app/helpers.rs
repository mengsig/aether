//! Application helpers.
use std::collections::{HashMap, HashSet};

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use aether_core::schedule::Schedule;

use super::{
    AgentStatusKind, AgentTree, Artifact, AuthInfo, ClientError, EventQuery, OpenArtifact, Theme,
    Then, Timestamp,
};

/// The name behind an id the daemon wrote into free text: an agent's name,
/// or a task's objective. `None` for anything this client has not been told
/// about, so the id stays rather than becoming a wrong name. A free function
/// so a renderer holding only the tree can call it.
pub fn name_of_id(tree: &AgentTree, id: &str) -> Option<String> {
    for node in tree.nodes.values() {
        if node.agent.id.to_string() == id {
            return agent_display_name(tree, node.agent.id);
        }
        if let Some(task) = &node.task
            && task.id.to_string() == id
        {
            return Some(crate::format::one_line(&task.objective, 60));
        }
    }
    None
}

pub(crate) fn agent_display_name(tree: &AgentTree, id: AgentId) -> Option<String> {
    sibling_names(tree)
        .remove(&id)
        .or_else(|| tree.nodes.get(&id).map(|node| node.agent.name.clone()))
}

/// Names that tell same-named siblings apart: under one parent (or among the
/// roots) the second `worker` is `worker 2`, the third `worker 3`, in creation
/// order; the first stays plain. Only renamed agents are in the map. Archived
/// siblings count, so a number never moves when they are shown or hidden.
pub(crate) fn sibling_names(tree: &AgentTree) -> HashMap<AgentId, String> {
    let groups = std::iter::once(&tree.roots).chain(tree.nodes.values().map(|n| &n.children));
    let mut names = HashMap::new();
    for ids in groups {
        let mut siblings: Vec<&Agent> = ids
            .iter()
            .filter_map(|id| tree.nodes.get(id))
            .map(|n| &n.agent)
            .collect();
        if siblings.len() < 2 {
            continue;
        }
        siblings.sort_by_key(|a| (a.created_at, a.id));
        // A sibling literally named `worker 2` keeps that name; the suffix skips it.
        let mut taken: HashSet<String> = siblings.iter().map(|a| a.name.clone()).collect();
        let mut seen: HashMap<&str, u32> = HashMap::new();
        for agent in siblings {
            let count = seen.entry(agent.name.as_str()).or_insert(0);
            *count += 1;
            if *count == 1 {
                continue;
            }
            let mut n = *count;
            while taken.contains(&format!("{} {n}", agent.name)) {
                n += 1;
            }
            let name = format!("{} {n}", agent.name);
            taken.insert(name.clone());
            names.insert(agent.id, name);
        }
    }
    names
}

/// Changes exactly when [`name_of_id`] or a schedule's name could answer differently:
/// a layout that replaced ids keys on it, so a rename or a new task re-lays it.
pub fn names_version(tree: &AgentTree, schedules: &[Schedule]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    for (id, node) in &tree.nodes {
        id.hash(&mut hasher);
        node.agent.name.hash(&mut hasher);
        node.agent.role.0.hash(&mut hasher);
        if let Some(task) = &node.task {
            task.id.hash(&mut hasher);
            task.objective.hash(&mut hasher);
        }
    }
    for job in schedules {
        job.id.hash(&mut hasher);
        job.name.hash(&mut hasher);
    }
    hasher.finish()
}

/// What to say instead of an empty viewer: when known, and why. §9.1
/// forbids a blank screen standing in for a failure.
pub(super) fn eviction_notice(a: &Artifact, now: Timestamp) -> String {
    match a.evicted_at {
        Some(at) => format!(
            "the bytes of that artifact were dropped by retention {} ago",
            crate::format::relative(at, now)
        ),
        None => "the bytes of that artifact were dropped".to_owned(),
    }
}

/// Decode an artifact payload for the viewer. Bytes that are not UTF-8 are
/// described rather than painted: a binary artifact rendered as replacement
/// characters says nothing and costs a screen.
pub(super) fn decode_artifact(artifact: Artifact, base64: &str, truncated: bool) -> OpenArtifact {
    use base64::Engine as _;
    let text = match base64::engine::general_purpose::STANDARD.decode(base64) {
        Err(e) => format!("the payload did not decode: {e}"),
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => format!(
                "{} of binary content ({})",
                crate::format::bytes(e.as_bytes().len() as u64),
                artifact.content_type.as_deref().unwrap_or("unknown type")
            ),
        },
    };
    OpenArtifact {
        artifact,
        text,
        truncated,
    }
}

/// A failed request in the person's terms: what went wrong, and what to do
/// about it (§9.9). The original error is always kept as the cause.
pub(super) fn explain(theme: &Theme, error: &ClientError, then: Then, command: &str) -> String {
    let text = error.to_string();
    if command.contains("--remote ")
        && matches!(
            error,
            ClientError::Incompatible(_) | ClientError::Disconnected | ClientError::Io(_)
        )
    {
        return format!(
            "{text}; check SSH connectivity and the Aether versions on the host and this client"
        );
    }
    // Only this code means nothing can run yet; any other model error, such
    // as a retired or misspelled one, falls through with the daemon's text.
    let no_model = matches!(error, ClientError::Protocol(reply) if reply.code == aether_protocol::ErrorCode::NoModel);
    if matches!(then, Then::AgentCreated(_)) && no_model {
        return theme
            .decoration(
                "no model to run on yet · /login to sign in, then send a message",
                "no model to run on yet | /login to sign in, then send a message",
            )
            .into();
    }
    let next = match error {
        ClientError::Incompatible(_) => {
            crate::ui_format!(
                theme,
                "the daemon is a different version · `{command} stop`, then start it again",
                "the daemon is a different version | `{command} stop`, then start it again"
            )
        }
        ClientError::Disconnected | ClientError::Io(_) => {
            crate::ui_format!(
                theme,
                "lost the daemon · reconnecting · `{command} daemon` starts one",
                "lost the daemon | reconnecting | `{command} daemon` starts one"
            )
        }
        ClientError::Unauthorized(_) => {
            crate::ui_format!(
                theme,
                "the token this daemon wants is not the one you gave · check AETHER_TOKEN",
                "the token this daemon wants is not the one you gave | check AETHER_TOKEN"
            )
        }
        ClientError::Protocol(_) => crate::ui_format!(
            theme,
            "the daemon refused it · /doctor shows what is set up",
            "the daemon refused it | /doctor shows what is set up"
        ),
        ClientError::DaemonStartup(_) => {
            crate::ui_format!(
                theme,
                "the daemon stopped as it started · the reason above is its own · fix that, then \
             start it again",
                "the daemon stopped as it started | the reason above is its own | fix that, then \
             start it again"
            )
        }
        // An older build never greets, but neither does a daemon whose workers are all blocked.
        ClientError::TimedOut {
            command: request, ..
        } if request == "hello" => {
            crate::ui_format!(
                theme,
                "the daemon never answered · it may be overloaded or from an older build · \
             `{command} stop`, then start it again",
                "the daemon never answered | it may be overloaded or from an \
             older build | `{command} stop`, then start it again"
            )
        }
        ClientError::TimedOut { .. } => {
            crate::ui_format!(
                theme,
                "the daemon is connected but slow · it may be busy on a loaded disk · try again; \
             if it keeps happening, `{command} stop`, then start it again",
                "the daemon is connected but slow | \
             it may be busy on a loaded disk | try again; if it keeps happening, `{command} stop`, then start it again"
            )
        }
        ClientError::Codec(_) | ClientError::Unexpected { .. } | ClientError::Malformed(_) => {
            crate::ui_format!(
                theme,
                "the daemon said something unexpected · /doctor, and the log in ~/.local/state/aether",
                "the daemon said something unexpected | /doctor, and the log in ~/.local/state/aether"
            )
        }
        ClientError::Ssh(_) | ClientError::DeliveryUnconfirmed(_) => {
            "check the host connection and refresh state before retrying".into()
        }
        ClientError::InvalidUsage(_) => crate::ui_format!(
            theme,
            "client request error · report this with the log in ~/.local/state/aether",
            "client request error | report this with the log in ~/.local/state/aether"
        ),
    };

    crate::ui_format!(theme, "{text} · {next}", "{text} | {next}")
}

/// What a reply was for, in the person's terms, for the disconnected-reply
/// notice. Kept separate from `explain`, which is about the
/// error rather than the request it answers.
pub(super) fn then_operation(then: &Then) -> &'static str {
    match then {
        Then::RemoteAck { next, .. } => then_operation(next),
        Then::Host(_) => "host workspace service",
        Then::RootDefaults { .. } | Then::AgentCreated(_) => "starting the agent",
        Then::Ignore => "that request",
        Then::CompactionAccepted(_) => "requesting compaction",
        Then::VoiceReply(_) => "sending a voice reply",
        Then::Status | Then::StatusPoll => "refreshing status",
        Then::Tree => "refreshing the agent tree",
        Then::Backfill { .. } => "loading the transcript",
        Then::Restarted { .. } => "restarting the agent",
        Then::Woken => "waking the agent",
        Then::FannedOut => "sending to the subtree",
        Then::Approvals { .. } => "loading approvals",
        Then::ApprovalRules => "loading approval rules",
        Then::McpServers => "loading MCP servers",
        Then::Memories => "loading memories",
        Then::MemoryMutated(_) => "updating the memory",
        Then::MemoryCreated { .. } => "admitting the operator memory",
        Then::AgentRenamed(_) => "renaming the agent",
        Then::Models => "loading models",
        Then::Notifications | Then::ResolvedNotifications => "loading notifications",
        Then::NotificationChanged => "updating the notification",
        Then::NotificationAnswered => "answering the notification",
        Then::Providers => "loading providers",
        Then::Schedules => "loading schedules",
        Then::ScheduleChanged => "updating the schedule",
        Then::ScheduleHistory(_) => "loading schedule history",
        Then::Usage => "loading usage",
        Then::RunReport(_) => "loading the run report",
        Then::Invocations(_) => "loading tool calls",
        Then::Artifacts(_) => "loading artifacts",
        Then::ArtifactBytes { .. } => "downloading the artifact",
        Then::AgentPatched => "updating the agent",
        Then::ModelPatched(_) => "changing the model",
        Then::PendingRootModelPatched(_) => "changing the new agent's model",
        Then::Auth => "loading sign-in status",
        Then::Interrupted(_) => "interrupting the agent",
        Then::Paused(_) => "pausing the agent",
        Then::Resumed(_) => "resuming the agent",
        Then::ApprovalResolved(_) => "answering the approval",
        Then::RuleRevoked => "revoking the approval rule",
        Then::ApprovalsSet { .. } => "changing approvals",
        Then::BoardConfirm { .. } => "checking affected agents",
        Then::BoardStopped { .. } => "stopping the agent",
        Then::BoardArchived { archived: true, .. } => "removing the agent",
        Then::BoardArchived {
            archived: false, ..
        } => "restoring the agent",
        Then::Archived => "archiving the agent",
        Then::QueuedSent(_) => "sending the queued message",
    }
}

/// The field of a tool call a person would recognise it by.
pub fn tool_input_hint(input: &serde_json::Value) -> Option<&str> {
    let obj = input.as_object()?;
    for key in [
        "command",
        "cmd",
        "path",
        "file_path",
        "pattern",
        "query",
        "objective",
        "to",
        "content",
    ] {
        if let Some(v) = obj.get(key).and_then(serde_json::Value::as_str) {
            return Some(v);
        }
    }
    obj.values().find_map(serde_json::Value::as_str)
}

/// VISION §9.6's glyph table, and the only place a status becomes one.
pub fn status_glyph(kind: AgentStatusKind) -> &'static str {
    match kind {
        AgentStatusKind::Created => "\u{25cc}",
        AgentStatusKind::Ready => "\u{25cb}",
        AgentStatusKind::Running => "\u{25cf}",
        AgentStatusKind::Waiting => "\u{25d0}",
        AgentStatusKind::Blocked => "\u{26a0}",
        AgentStatusKind::Completed => "\u{2713}",
        AgentStatusKind::Failed => "\u{2717}",
        AgentStatusKind::Cancelled => "\u{2298}",
        AgentStatusKind::Imported => "\u{2913}",
    }
}

pub fn subscription_query() -> EventQuery {
    EventQuery::default()
}

/// One displayable piece of a durable user message.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxEntry {
    /// Something a person typed; shown as a user bubble.
    User(String),
    /// Runtime delivery (task completed, approval, wake); shown as a note.
    Note(String),
}

/// Durable user turns carry the inbox rendering the model saw. Split it back
/// into what the person typed and what the runtime delivered, so the chat
/// shows one clean bubble per message instead of the wire text.
pub fn split_inbox_text(text: &str) -> Vec<InboxEntry> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if !text.starts_with("[inbox ") {
        return vec![InboxEntry::User(text.to_owned())];
    }
    let mut out = Vec::new();
    let mut current: Option<(bool, String)> = None;
    let flush = |current: &mut Option<(bool, String)>, out: &mut Vec<InboxEntry>| {
        if let Some((is_user, body)) = current.take() {
            let body = body.trim().to_owned();
            if body.is_empty() {
                return;
            }
            out.push(if is_user {
                InboxEntry::User(body)
            } else {
                InboxEntry::Note(body)
            });
        }
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("[inbox ") {
            flush(&mut current, &mut out);
            let after = rest.split_once("] ").map(|(_, r)| r).unwrap_or(rest);
            if let Some(from) = after.strip_prefix("Message from ") {
                let from = from.trim_end_matches(':');
                if from == "user" || from.starts_with("user:") {
                    current = Some((true, String::new()));
                } else {
                    current = Some((false, format!("message from {from}: ")));
                }
            } else {
                current = Some((false, after.to_owned()));
            }
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            if !body.is_empty() && !body.ends_with(' ') {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    flush(&mut current, &mut out);
    out
}

/// Install command for a provider whose CLI is missing.
pub fn install_command(provider: &str) -> Option<&'static str> {
    match provider {
        "claude-code" => Some("npm install -g @anthropic-ai/claude-code"),
        "codex" => Some("npm install -g @openai/codex"),
        _ => None,
    }
}

pub fn cli_missing(info: &AuthInfo) -> bool {
    info.program.is_some()
        && info
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("not found"))
}
