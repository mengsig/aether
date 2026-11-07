//! The one dispatch table: which part of the service answers each Noesis
//! command. A command missing here fails the dispatch-table test.

use aether_protocol::Command;

/// The handler group that owns a Noesis command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    /// `noesis_status` and `noesis_configure`, answered while disabled (F3).
    Service,
    /// The learner model's reads and correction (F4).
    Learner,
    /// Sessions, learner input and the teaching loop (F3b).
    Teaching,
    /// The knowledge graph, sources and extraction jobs: F8 plugs in here.
    Graph,
}

/// The group that answers `command`. Panics in debug builds for a command
/// that is not a Noesis command; release builds route it to the service.
pub(crate) fn group_of(command: &Command) -> Group {
    debug_assert!(command.is_noesis(), "only noesis commands are routed");
    match command {
        Command::NoesisStatus | Command::NoesisConfigure { .. } => Group::Service,
        Command::NoesisLearner { .. }
        | Command::NoesisLearnerEvidence { .. }
        | Command::NoesisCorrectLearner { .. } => Group::Learner,
        Command::NoesisCreateSession { .. }
        | Command::NoesisSend { .. }
        | Command::NoesisAnswer { .. }
        | Command::NoesisGetSession { .. }
        | Command::NoesisListSessions { .. }
        | Command::NoesisDeleteSession { .. } => Group::Teaching,
        Command::NoesisGraph { .. }
        | Command::NoesisDeleteProposition { .. }
        | Command::NoesisDeleteSource { .. }
        | Command::NoesisExtract { .. }
        | Command::NoesisImportSource { .. }
        | Command::NoesisJobs { .. }
        | Command::NoesisCancelJob { .. } => Group::Graph,
        _ => Group::Service,
    }
}
