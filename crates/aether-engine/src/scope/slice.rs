//! `aether-agents.slice`, which every engine scope joins. The daemon names its
//! limits at start; they are set on the first scope, so a daemon that runs no
//! engine never asks systemd. A failure leaves engines where they ran before.
//! All off resets what an earlier daemon set, and scopes stay out of the slice.

use aether_core::agent_slice::AgentSliceLimits;

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// No daemon named limits: scopes go where they always went.
    Unset,
    Pending(AgentSliceLimits),
    Ready(AgentSliceLimits),
    Failed(AgentSliceLimits, String),
}

/// A slice of agents' engines under its parent, and how far its limits got.
pub(super) struct AgentSlice {
    parent: &'static str,
    slice: &'static str,
    state: std::sync::Mutex<State>,
}

/// Process-wide like the scopes themselves: one user manager, one slice.
static AGENTS: AgentSlice = AgentSlice::new(
    aether_core::agent_slice::AGENT_SLICE_PARENT,
    aether_core::agent_slice::AGENT_SLICE,
);

/// The limits engine scopes' shared slice carries from the next scope on.
/// All off resets the slice and keeps scopes out of it.
pub fn set_agent_slice(limits: AgentSliceLimits) {
    AGENTS.set(limits);
}

/// What `aether status` shows of the slice; `None` where there is none to
/// show: off Linux, or in a process that named no limits.
pub fn agent_slice_status() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    #[cfg(target_os = "linux")]
    if super::linux::disabled() {
        return Some("off (AETHER_ENGINE_SCOPE=0)".to_owned());
    }
    AGENTS.status()
}

/// The slice a new engine scope joins, set up first if it is not yet.
#[cfg(target_os = "linux")]
pub(super) async fn joined() -> Option<&'static str> {
    AGENTS.joined().await
}

impl AgentSlice {
    pub(super) const fn new(parent: &'static str, slice: &'static str) -> Self {
        Self {
            parent,
            slice,
            state: std::sync::Mutex::new(State::Unset),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // Every value of the state is valid whatever panicked while holding it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn set(&self, limits: AgentSliceLimits) {
        *self.state() = State::Pending(limits);
    }

    pub(super) fn status(&self) -> Option<String> {
        describe(self.slice, &self.state())
    }

    /// Where a scope goes once `limits` are set: nowhere new when all are off.
    fn placement(&self, limits: &AgentSliceLimits) -> Option<&'static str> {
        (!limits.is_off()).then_some(self.slice)
    }

    /// Set the pending limits once, at the first scope; the slice to join.
    #[cfg(target_os = "linux")]
    pub(super) async fn joined(&self) -> Option<&'static str> {
        // Concurrent first scopes set the slice once; the others wait for it.
        static APPLYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let pending = || match &*self.state() {
            State::Pending(limits) => Ok(limits.clone()),
            State::Ready(limits) => Err(self.placement(limits)),
            State::Unset | State::Failed(..) => Err(None),
        };
        if let Err(settled) = pending() {
            return settled;
        }
        let _applying = APPLYING.lock().await;
        let limits = match pending() {
            Ok(limits) => limits,
            Err(settled) => return settled,
        };
        let applied = apply(self.parent, self.slice, &limits).await;
        let mut state = self.state();
        // Limits named again meanwhile are set by the next scope.
        if *state != State::Pending(limits.clone()) {
            return None;
        }
        let slice = self.slice;
        match applied {
            Ok(()) => {
                tracing::info!(%limits, "{slice} set; engines run in it unless all are off");
                let placed = self.placement(&limits);
                *state = State::Ready(limits);
                placed
            }
            Err(error) => {
                tracing::warn!(%limits, %error, "cannot set {slice}; engines run outside it");
                *state = State::Failed(limits, error);
                None
            }
        }
    }
}

fn describe(slice: &str, state: &State) -> Option<String> {
    let shown = match state {
        State::Unset => return None,
        State::Pending(limits) if limits.is_off() => {
            "off (earlier limits are reset at the first engine)".to_owned()
        }
        State::Ready(limits) if limits.is_off() => "off".to_owned(),
        State::Failed(limits, error) if limits.is_off() => {
            format!("off; earlier limits on {slice} not reset ({error})")
        }
        State::Pending(limits) => format!("{slice} {limits} (set at the first engine)"),
        State::Ready(limits) => format!("{slice} {limits}"),
        State::Failed(limits, error) => {
            format!("{slice} {limits} not set ({error}); engines run outside it")
        }
    };
    Some(shown)
}

/// Set `limits` on `slice` and its weights on `parent`, at runtime only, so
/// a reboot or a user manager restart forgets them. Off limits are reset.
#[cfg(target_os = "linux")]
pub(super) async fn apply(
    parent: &str,
    slice: &str,
    limits: &AgentSliceLimits,
) -> Result<(), String> {
    assert!(
        slice
            .strip_suffix(".slice")
            .and_then(|s| s.rsplit_once('-'))
            == parent.strip_suffix(".slice").map(|p| (p, "agents")),
        "{slice} is not the agents' slice under {parent}"
    );
    let parent_args = set_property_args(parent, limits.weight_properties());
    let slice_args = set_property_args(slice, limits.slice_properties());
    let parent_args: Vec<&str> = parent_args.iter().map(String::as_str).collect();
    let slice_args: Vec<&str> = slice_args.iter().map(String::as_str).collect();
    let (parent_set, slice_set) = tokio::join!(
        super::linux::systemctl(&parent_args),
        super::linux::systemctl(&slice_args),
    );
    parent_set.and(slice_set).map(drop)
}

#[cfg(target_os = "linux")]
pub(super) fn set_property_args(unit: &str, properties: Vec<String>) -> Vec<String> {
    assert!(unit.ends_with(".slice"), "not a slice: {unit}");
    assert!(
        properties.iter().all(|p| p.contains('=')),
        "not assignments: {properties:?}"
    );
    let mut args = vec![
        "set-property".to_owned(),
        "--runtime".to_owned(),
        unit.to_owned(),
    ];
    args.extend(properties);
    args
}

#[cfg(test)]
mod tests {
    use super::{State, describe};
    use aether_core::agent_slice::AgentSliceLimits;

    #[test]
    fn status_says_where_engines_run() {
        let limits = AgentSliceLimits::default();
        let off = AgentSliceLimits {
            cpu_weight: None,
            io_weight: None,
            memory_high: None,
        };
        assert_eq!(describe("aether-agents.slice", &State::Unset), None);
        assert_eq!(
            describe("aether-agents.slice", &State::Pending(off.clone())).as_deref(),
            Some("off (earlier limits are reset at the first engine)")
        );
        assert_eq!(
            describe("aether-agents.slice", &State::Ready(off.clone())).as_deref(),
            Some("off")
        );
        let unreset =
            describe("aether-agents.slice", &State::Failed(off, "no bus".into())).unwrap();
        assert_eq!(
            unreset,
            "off; earlier limits on aether-agents.slice not reset (no bus)"
        );
        assert_eq!(
            describe("aether-agents.slice", &State::Pending(limits.clone())).as_deref(),
            Some(
                "aether-agents.slice CPUWeight=20 IOWeight=20 MemoryHigh=8G (set at the first engine)"
            )
        );
        assert_eq!(
            describe("aether-agents.slice", &State::Ready(limits.clone())).as_deref(),
            Some("aether-agents.slice CPUWeight=20 IOWeight=20 MemoryHigh=8G")
        );
        let failed = describe(
            "aether-agents.slice",
            &State::Failed(limits, "no bus".into()),
        )
        .unwrap();
        assert!(
            failed.ends_with("not set (no bus); engines run outside it"),
            "{failed}"
        );
    }
}
