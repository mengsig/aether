//! The end of a run: where it got to and what to type next.

use super::daemon::Daemon;
use super::environment::Environment;
use super::guidance;
use super::report::{Status, StepId, StepState};
use super::runner::CommandRunner;
use super::ui::Ui;
use super::wizard::Wizard;

impl<R: CommandRunner, D: Daemon, E: Environment, U: Ui> Wizard<'_, R, D, E, U> {
    /// Sets the report's `next` commands and prints the closing summary.
    pub(super) fn finish(&mut self) {
        assert!(
            !self.report.steps.is_empty(),
            "a run reports at least one step"
        );
        let paired = self
            .report
            .step(StepId::Pairing)
            .is_some_and(|step| step.state == StepState::Changed || step.state == StepState::Done);
        self.report.next = match self.report.status {
            Status::Complete => Self::complete_next(paired),
            Status::Pending => vec!["aether remote setup --yes".to_owned()],
            Status::Blocked | Status::Stopped => vec!["aether remote setup".to_owned()],
        };
        assert!(
            self.report.status != Status::Complete
                || self.report.steps.iter().all(|step| !matches!(
                    step.state,
                    StepState::Blocked | StepState::Stopped | StepState::Pending
                )),
            "a complete run has an unfinished step"
        );
        self.print_summary();
    }

    fn complete_next(paired: bool) -> Vec<String> {
        let mut next = Vec::new();
        if !paired {
            next.push("aether remote pair".to_owned());
        }
        next
    }

    fn print_summary(&mut self) {
        self.heading("Step 5 of 5: where things stand");
        let steps = self.report.steps.clone();
        for step in &steps {
            self.say(&format!(
                "  {:<9} {:?}: {}",
                format!("{:?}", step.id),
                step.state,
                step.detail
            ));
        }
        match self.report.status {
            Status::Complete => {
                self.say("\nRemote access is on and private to your tailnet.");
                if let Some(origin) = self.report.origin.clone() {
                    self.say(&format!("Address: {origin}"));
                }
                self.say("\nManage it:");
                for (command, what) in guidance::management_commands() {
                    self.say(&format!("  {command:<28} {what}"));
                }
            }
            Status::Pending => self.say("\nNothing was changed. Run `aether remote setup --yes` to make these changes."),
            Status::Blocked => self.say("\nSetup stopped at the step above. Fix it and run `aether remote setup` again: it skips what is done."),
            Status::Stopped => self.say("\nStopped. Run `aether remote setup` again whenever you like: it skips what is done."),
        }
    }
}
