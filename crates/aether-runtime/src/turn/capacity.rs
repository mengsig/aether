use super::*;

/// The effective cap over an agent's whole subtree (VISION §3.6 limit 3):
/// its own `concurrency` if it set one, else the daemon's global limit,
/// since that is what bounds it either way - and which one that is, so a
/// caller can say so.
pub(super) fn effective_cap(runtime: &Runtime, agent: &Agent) -> (usize, &'static str) {
    match agent.config.concurrency {
        Some(cap) => (cap, "your concurrency cap"),
        None => (
            runtime.config.max_concurrent_turns,
            "the daemon's global concurrency limit",
        ),
    }
}

/// How a `delegate` reply or the inspector describes an agent's subtree cap
/// against what is actually running under it. `Counted` matches exactly what
/// the scheduler enforces (`scheduler::on_provider_descendants`); `Unknown`
/// is the fleet read failing, named rather than swallowed (this
/// read is advisory and must never fail the turn - `?` on it after
/// `create_agent` already committed the child would strand a recorded
/// delegation with no result).
pub(super) enum CapStatus {
    Counted {
        running: usize,
        cap: usize,
        source: &'static str,
        queued: bool,
    },
    Unknown {
        cap: usize,
        source: &'static str,
        error: String,
    },
}

/// A short class for a store error safe to put in a tool result or an
/// event, never its `Display` text: a sqlite error routinely carries the
/// database path, and this text reaches the model.
pub(super) fn store_error_class(e: &StoreError) -> &'static str {
    match e {
        StoreError::Sqlite(_) => "a database error",
        StoreError::Io(_) => "an I/O error",
        StoreError::MigrationBackup { .. } => "a pre-migration backup failure",
        StoreError::Serde(_) => "a serialization error",
        StoreError::Corrupt(_) => "store corruption",
        StoreError::SchemaTooNew { .. } => "a schema version mismatch",
        StoreError::InboxSequence { .. } => "an inbox sequence mismatch",
        StoreError::NoArtifactDir => "no artifact directory configured",
        StoreError::HistoryExpired { .. } => "requested event history has expired",
        StoreError::RetentionSnapshotTooLarge { .. } => {
            "retention snapshot exceeds its memory limit"
        }
        StoreError::AliasRefused(_) => "a refused repository alias",
    }
}

/// The advisory told to whoever calls `delegate`: which cap, if any, the
/// scheduler will actually hold the new child against. Derived from
/// `scheduler::cap_holder`, the same ancestor-and-global-limit predicate
/// `dispatch` admits against, so `queued` never disagrees with what starts
/// the child (the `None`-cap default and an inherited ancestor cap were
/// once both compared against the wrong count).
/// `child_on_engine` says whether the child just created runs on an engine,
/// so the daemon's engine cap is reported only where it actually applies.
pub(super) fn cap_status(runtime: &Runtime, agent: &Agent, child_on_engine: bool) -> CapStatus {
    let engine_cap = runtime
        .config
        .max_concurrent_engine_turns
        .filter(|_| child_on_engine);
    match runtime.store.agents(&AgentFilter::default()) {
        Ok(live) => {
            let sched = runtime.sched.lock();
            match crate::scheduler::cap_holder(
                agent.id,
                &live,
                &sched,
                runtime.config.max_concurrent_turns,
                engine_cap,
            ) {
                crate::scheduler::CapHolder::Spent {
                    agent: holder,
                    running,
                    cap,
                } => {
                    let source = match holder {
                        Some(id) if id == agent.id => "your concurrency cap",
                        Some(_) => "an ancestor's concurrency cap",
                        None => "the daemon's global concurrency limit",
                    };
                    CapStatus::Counted {
                        running,
                        cap,
                        source,
                        queued: true,
                    }
                }
                crate::scheduler::CapHolder::EngineCap { running, cap } => CapStatus::Counted {
                    running,
                    cap,
                    source: "the daemon's engine-turn cap",
                    queued: true,
                },
                crate::scheduler::CapHolder::Clear => {
                    let (cap, source) = effective_cap(runtime, agent);
                    let running =
                        crate::scheduler::on_provider_descendants(agent.id, &live, &sched);
                    CapStatus::Counted {
                        running,
                        cap,
                        source,
                        queued: false,
                    }
                }
            }
        }
        Err(e) => {
            let (cap, source) = effective_cap(runtime, agent);
            // The full error (a sqlite path, routinely) is logged for an
            // operator, never sent to the model or the event log: a
            // harness action's outcome text skips the outcome redactor.
            tracing::warn!(agent = %agent.id, error = %e, "cap advisory: could not read the fleet");
            CapStatus::Unknown {
                cap,
                source,
                error: store_error_class(&e).to_owned(),
            }
        }
    }
}
