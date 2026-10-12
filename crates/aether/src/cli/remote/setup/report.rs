//! What a run did, step by step: printed as `--json`, and the source of the
//! closing summary.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    /// A person is answering questions.
    Interactive,
    /// `--yes`: defaults are accepted and nobody is waited for.
    Yes,
    /// `--json` without `--yes`: look, change nothing.
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepId {
    Tailscale,
    Daemon,
    Listener,
    Publish,
    Verify,
    Pairing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepState {
    /// Already as wanted; nothing was changed.
    Done,
    /// Was not as wanted; this run changed it.
    Changed,
    /// Not needed or not possible in this run.
    Skipped,
    /// Works, with something to know.
    Warning,
    /// A check-only run found something it would change.
    Pending,
    /// Cannot go on until a person or the system acts.
    Blocked,
    /// The person chose to stop.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Step {
    pub(super) id: StepId,
    pub(super) state: StepState,
    pub(super) detail: String,
    /// The command that does or would do it, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) command: Option<String>,
    /// What to do about a step that did not finish.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) fix: Vec<String>,
}

impl Step {
    pub(super) fn new(id: StepId, state: StepState, detail: impl Into<String>) -> Self {
        Self {
            id,
            state,
            detail: detail.into(),
            command: None,
            fix: Vec::new(),
        }
    }

    pub(super) fn command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    pub(super) fn fix(mut self, fix: Vec<String>) -> Self {
        self.fix = fix;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    /// Remote access works end to end.
    Complete,
    /// Something stands in the way.
    Blocked,
    /// A check-only run: these changes would be made.
    Pending,
    /// The person stopped it.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Report {
    pub(super) status: Status,
    pub(super) mode: Mode,
    pub(super) platform: String,
    /// The address phones use, once known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) origin: Option<String>,
    pub(super) steps: Vec<Step>,
    /// Commands that carry on from here.
    pub(super) next: Vec<String>,
}

impl Report {
    pub(super) fn new(mode: Mode, platform: String) -> Self {
        Self {
            status: Status::Complete,
            mode,
            platform,
            origin: None,
            steps: Vec::new(),
            next: Vec::new(),
        }
    }

    /// Adds `step` and returns its state.
    pub(super) fn push(&mut self, step: Step) -> StepState {
        let state = step.state;
        assert!(
            self.step(step.id).is_none(),
            "step {:?} was reported twice",
            step.id
        );
        assert!(!step.detail.is_empty(), "a step must say what happened");
        self.steps.push(step);
        self.status = self.worst();
        state
    }

    pub(super) fn step(&self, id: StepId) -> Option<&Step> {
        self.steps.iter().find(|step| step.id == id)
    }

    fn worst(&self) -> Status {
        let any = |state| self.steps.iter().any(|step| step.state == state);
        if any(StepState::Blocked) {
            Status::Blocked
        } else if any(StepState::Stopped) {
            Status::Stopped
        } else if any(StepState::Pending) {
            Status::Pending
        } else {
            Status::Complete
        }
    }

    pub(super) fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}
